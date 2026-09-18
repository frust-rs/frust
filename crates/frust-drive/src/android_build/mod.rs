//! Android release build pipeline: preflight (reusing `android_run`'s
//! checks, minus the device-only `adb` one) → merge-write
//! `android/local.properties` → gate release builds on a keystore and hand
//! the resolved material to Gradle as a short-lived
//! `android/.frust-signing.properties` → compute
//! the `assemble<Flavor><Mode>`/`bundle<Flavor><Mode>` Gradle task and its
//! `-P` properties → run `./gradlew` → glob-verify and report the produced
//! artifact(s).

// `pub(crate)`, not private: `android_run::run` reuses these helpers
// directly (task-name/`-P`-property computation, the local.properties
// merge-write, the release-signing gate, artifact discovery) rather than
// duplicating the naming/path logic for `frust run`'s Android pipeline.
pub(crate) mod artifacts;
pub(crate) mod local_properties;
pub(crate) mod signing;
pub(crate) mod tasks;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::android_run::preflight::{self, PreflightCtx};
use crate::build_info::BuildInfo;
use crate::doctor::{EnvLookup, RealEnv};
use crate::process::{ProcessRunner, tail_lines};

/// The artifact `frust build apk`/`appbundle` requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndroidArtifact {
    /// A `--split-per-abi` (one APK per ABI) or fat APK build, for the
    /// already-mapped Gradle ABI names (`arm64-v8a`, `armeabi-v7a`,
    /// `x86_64`) requested via `--target-platform`.
    Apk {
        split_per_abi: bool,
        abis: Vec<String>,
    },
    /// An Android App Bundle (`.aab`) for Play Store distribution.
    Appbundle,
}

/// Paths to the artifact(s) a successful build produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuiltArtifacts {
    pub paths: Vec<PathBuf>,
}

/// Drives the Android release pipeline (Gradle `assemble<Flavor><Mode>` /
/// `bundle<Flavor><Mode>`, `cargo ndk` under the hood) for `target` in the
/// Frust project rooted at `project_dir`.
///
/// **Print-free core.** Every line this pipeline would surface (streamed
/// `[gradle] …` output, the produced-artifact size notes) is emitted through
/// `on_line`, never `println!` — so a caller holding a raw-mode terminal (the
/// `frust-tui` build session) can route it into a log tab instead of leaking
/// it to the tty. The CLI (`commands::build`) passes an `on_line` that just
/// `println!`s each line, preserving its stdout verbatim.
///
/// `extra_features` is the front-end's `--features` passthrough, appended to
/// the mode's own cargo features and carried to `cargo ndk` inside the same
/// base64 `-Pfrust.cargoFeatures` property (see
/// [`tasks::gradle_properties`]); an empty slice reproduces the pre-passthrough
/// invocation exactly. `frust-tui`'s build session has no flag surface for it
/// and passes an empty slice.
///
/// **Frozen signature** — do not change without updating every caller
/// (`commands::build`, `frust-tui`'s build session) and this doc comment.
/// The `on_line` sink was added by the tty-garbling fix; `extra_features` by
/// the cargo-feature passthrough.
pub fn build(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    target: &AndroidArtifact,
    extra_features: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<BuiltArtifacts> {
    build_with_env(
        runner,
        project_dir,
        info,
        target,
        extra_features,
        &RealEnv,
        on_line,
    )
}

/// The testable core of [`build`], taking an injected [`EnvLookup`] so
/// `JAVA_HOME` resolution (`preflight::run_without_device_checks`) can be
/// exercised with a `crate::doctor::FakeEnv` in tests instead of the real
/// process environment — kept private since [`build`]'s frozen signature
/// has no room for a fifth parameter.
fn build_with_env(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    target: &AndroidArtifact,
    extra_features: &[String],
    env: &dyn EnvLookup,
    on_line: &mut dyn FnMut(&str),
) -> Result<BuiltArtifacts> {
    let android_dir = project_dir.join("android");

    // r1-04 replaced AGP's default jniLibs source set with the `build/`
    // redirect (setSrcDirs) in the generated template and every in-repo
    // example, so a leftover `android/app/src/main/jniLibs` is no longer
    // packaged into anything Gradle produces — flag it once so a project
    // that hasn't run `frust clean` since regenerating isn't left wondering
    // why its native libs still sit there.
    artifacts::warn_if_legacy_jni_libs(project_dir, on_line);

    let preflight_ctx = PreflightCtx {
        runner,
        env,
        is_macos: cfg!(target_os = "macos"),
    };
    let outcome =
        preflight::run_without_device_checks(&preflight_ctx).map_err(|err| anyhow::anyhow!(err))?;

    let version_name = info.build_name.clone().unwrap_or_else(|| "1.0".to_string());
    let version_code = info
        .build_number
        .map(|n| n.to_string())
        .unwrap_or_else(|| "1".to_string());
    local_properties::write(&android_dir, &version_name, &version_code)
        .context("writing android/local.properties")?;

    // One source of truth for signing: the gate resolves `[signing]`, and the
    // material it verified is handed to Gradle as
    // `android/.frust-signing.properties` (unprefixed keys, absolute
    // storeFile). The guard's `Drop` deletes that file the moment this
    // function returns — including every `?`/`bail!` path below — and the
    // `crate::interrupt` registration it carries (armed before the file is
    // created) covers the routes `Drop` cannot: a Ctrl-C or SIGTERM during the
    // multi-minute Gradle invocation below, and an abort. So the plaintext
    // passwords never outlive the Gradle invocation that needed them. Non-
    // release modes and `[signing] external = true` resolve to `None` and write
    // nothing.
    let generated = signing::check_release_signing(project_dir, info.mode, on_line)?
        .map(|resolved| signing::write_resolved(&android_dir, &resolved, on_line))
        .transpose()?;
    // `Some` iff this is a release build the gate actually vouched for — a
    // non-release mode and `[signing] external = true` both resolve to `None`.
    // Recorded before the guard is dropped below, since that is exactly the
    // condition under which Gradle's own verdict has to be checked.
    let signing_promised = generated.is_some();

    // Release-lean preflight: a legacy app that predates the `lean` feature
    // has it dropped here — with a one-time warning routed through this
    // print-free core's `on_line` sink — so `cargo ndk` is never handed an
    // undeclared `--features lean` (cargo's opaque hard error). A declaring
    // app keeps byte-identical features and warns nothing.
    let (features, warning) =
        crate::cargo_manifest::resolve_release_features(project_dir, info.mode, extra_features);
    if let Some(warning) = warning {
        on_line(&warning);
    }

    let task = tasks::task_name(target, info.mode, info.flavor.as_deref());
    let feature_refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let props = tasks::gradle_properties(target, &info.defines, &feature_refs);

    // The same leading `--project-cache-dir <project>/build/android/.gradle`
    // `android_run::gradle::assemble` passes, taken from that one place
    // rather than re-spelled here — the two Gradle lanes must not drift on
    // where Gradle's own cache goes (see `gradle::leading_args`).
    let leading = crate::android_run::gradle::leading_args(project_dir);
    let mut args: Vec<&str> = Vec::with_capacity(leading.len() + 1 + props.len());
    for arg in &leading {
        args.push(arg.as_str());
    }
    args.push(task.as_str());
    for prop in &props {
        args.push(prop.as_str());
    }

    let mut prefixed = |line: &str| on_line(&format!("[gradle] {line}"));
    let out = runner
        .run_streaming(
            "./gradlew",
            &args,
            Some(&android_dir),
            &[("JAVA_HOME", &outcome.java_home)],
            &mut prefixed,
        )
        .with_context(|| format!("running `./gradlew {task}` in `{}`", android_dir.display()))?;
    // Gradle has returned; the generated signing file has no further reader.
    // (An early `?` above drops it just the same — this only narrows the
    // window for the success path.)
    drop(generated);

    let combined = format!("{}\n{}", out.stdout, out.stderr);
    if !out.success {
        if let Some(flavor) = info.flavor.as_deref()
            && task_not_found(&combined, &task)
        {
            bail!(
                "flavor '{flavor}' has no Gradle product flavor — declare it in \
                 android/app/build.gradle.kts (see the commented example)"
            );
        }
        let tail = tail_lines(&out.stderr, 50);
        if tail.is_empty() {
            bail!("`./gradlew {task}` failed");
        }
        bail!("`./gradlew {task}` failed:\n{tail}");
    }

    // The gate promised a release-signed artifact; Gradle just said it produced
    // a debug-signed one. Key off what Gradle *did* rather than what the gate
    // predicted — see `signing`'s module doc ("The backstop"). Checked only on
    // a successful build: a failed one already fails, and Gradle can echo build
    // script text (which quotes the marker in a comment) while diagnosing.
    if signing_promised && signing::reported_debug_signing(&combined) {
        return Err(signing::debug_signed_error());
    }

    let paths = artifacts::discover(
        project_dir,
        target,
        info.mode,
        info.flavor.as_deref(),
        on_line,
    )?;
    for path in &paths {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        on_line(&format!("{} ({size} bytes)", path.display()));
    }

    Ok(BuiltArtifacts { paths })
}

/// Heuristically detects Gradle's "task not found" failure for `task`
/// (e.g. `Task 'assemblePaidRelease' not found in root project '…'`) in the
/// build's combined stdout+stderr, so a missing `--flavor` product flavor
/// can be re-explained instead of surfacing Gradle's generic message.
fn task_not_found(combined_output: &str, task: &str) -> bool {
    combined_output.contains(&format!("'{task}'")) && combined_output.contains("not found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::{BuildArgs, BuildMode};
    use crate::doctor::FakeEnv;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    const JAVA_HOME: &str = "/opt/jdk17";

    /// The `FakeProcessRunner` key for a Frust-driven `./gradlew` invocation
    /// in the project at `project_dir`: every one now leads with
    /// `--project-cache-dir <project>/build/android/.gradle` (see
    /// `android_run::gradle::leading_args`), so a fixture has to carry it
    /// too. Registering the whole argv — the fake only falls back to a
    /// registered key that is a *prefix* of the real one — is what makes
    /// these tests assert the argv: drop or misplace the cache flag and no
    /// fixture matches at all.
    fn gradlew_key(project_dir: &Path, task_and_props: &str) -> String {
        format!(
            "./gradlew --project-cache-dir {} {task_and_props}",
            crate::android_run::gradle::project_cache_dir(project_dir).display()
        )
    }

    fn unique_project_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-android-build-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("android")).unwrap();
        dir
    }

    /// Complete release signing material for a default-configured project:
    /// the four values `signing::check_release_signing` resolves, plus a
    /// placeholder file at the `storeFile` path they name (the gate checks
    /// that a keystore exists there, not that it is a valid JKS).
    fn write_release_signing(android_dir: &Path) {
        fs::write(android_dir.join("upload.jks"), b"not-a-real-jks").unwrap();
        fs::write(
            android_dir.join("key.properties"),
            "storePassword=pw\nkeyPassword=pw\nkeyAlias=upload\nstoreFile=upload.jks\n",
        )
        .unwrap();
    }

    /// No `--features` passthrough — what every caller but an explicit
    /// passthrough test passes, and byte-identical to the pre-passthrough
    /// invocation.
    const NO_EXTRA: &[String] = &[];

    fn fake_env() -> FakeEnv {
        FakeEnv::new().set("JAVA_HOME", JAVA_HOME)
    }

    fn preflight_ok_runner() -> FakeProcessRunner {
        FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                Output {
                    success: true,
                    stdout: "aarch64-linux-android\n".to_string(),
                    stderr: String::new(),
                },
            )
            .with(
                "cargo ndk --version",
                Output {
                    success: true,
                    stdout: "cargo-ndk 3.5.4\n".to_string(),
                    stderr: String::new(),
                },
            )
            .with(
                format!("{JAVA_HOME}/bin/java -version"),
                Output {
                    success: true,
                    stdout: String::new(),
                    stderr: "openjdk version \"17.0.9\" 2023-10-17\n".to_string(),
                },
            )
    }

    fn info(mode: BuildMode, flavor: Option<&str>) -> BuildInfo {
        BuildInfo::from_args(
            BuildArgs {
                flavor: flavor.map(str::to_string),
                ..BuildArgs::default()
            },
            mode,
        )
        .unwrap()
    }

    #[test]
    fn full_pipeline_debug_apk_no_flavor_asserts_exact_gradlew_invocation_and_artifacts() {
        let dir = unique_project_dir("full-debug-apk");
        let android_dir = dir.join("android");
        let out_dir = dir.join("build/android/app/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake-apk-bytes").unwrap();

        let runner = preflight_ok_runner().with(
            gradlew_key(
                &dir,
                "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            ),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Debug, None);
        let result = build_with_env(
            &runner,
            &dir,
            &build_info,
            &target,
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-debug.apk")]);

        // local.properties merge-write happened as a side effect.
        let local_props = fs::read_to_string(android_dir.join("local.properties")).unwrap();
        assert!(local_props.contains("frust.versionName=1.0"));
        assert!(local_props.contains("frust.versionCode=1"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// The legacy fixture for the `frust build` lane: a project whose
    /// generated Gradle config predates the `build/android/` redirect still
    /// writes its APK to AGP's own `android/app/build/outputs/…`. The build
    /// must still find it — and say so exactly once, naming the migration
    /// recipe — rather than failing on a directory that project was never
    /// going to write to. (The `--project-cache-dir` argument is unaffected:
    /// it comes off the command line, not out of the project's Gradle
    /// config, so even a pre-migration project gets its cache relocated.)
    #[test]
    fn full_pipeline_finds_a_pre_migration_apk_and_warns_once() {
        let dir = unique_project_dir("legacy-layout-apk");
        let out_dir = dir.join("android/app/build/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake-apk-bytes").unwrap();

        let runner = preflight_ok_runner().with(
            gradlew_key(
                &dir,
                "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            ),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let mut lines = Vec::new();
        let result = build_with_env(
            &runner,
            &dir,
            &info(BuildMode::Debug, None),
            &arm64_apk(),
            NO_EXTRA,
            &fake_env(),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-debug.apk")]);

        let warnings: Vec<&String> = lines
            .iter()
            .filter(|l| l.contains("pre-migration path"))
            .collect();
        assert_eq!(warnings.len(), 1, "{lines:?}");
        assert!(
            warnings[0].contains(artifacts::MIGRATION_RECIPE_DOC),
            "{warnings:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// The build lane's half of the jniLibs Major: a leftover
    /// `android/app/src/main/jniLibs` (r1-04 replaced AGP's default source
    /// set, so this directory is never packaged into anything Gradle
    /// produces anymore) is warned about exactly once, naming the migration
    /// recipe — the build itself still succeeds.
    #[test]
    fn full_pipeline_warns_once_about_a_legacy_jni_libs_leftover() {
        let dir = unique_project_dir("legacy-jnilibs");
        let out_dir = dir.join("build/android/app/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake-apk-bytes").unwrap();
        let jni_dir = dir.join("android/app/src/main/jniLibs/arm64-v8a");
        fs::create_dir_all(&jni_dir).unwrap();
        fs::write(jni_dir.join("libapp.so"), b"stale").unwrap();

        let runner = preflight_ok_runner().with(
            gradlew_key(
                &dir,
                "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            ),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let mut lines = Vec::new();
        let result = build_with_env(
            &runner,
            &dir,
            &info(BuildMode::Debug, None),
            &arm64_apk(),
            NO_EXTRA,
            &fake_env(),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-debug.apk")]);

        let warnings: Vec<&String> = lines
            .iter()
            .filter(|l| l.contains("jniLibs is a pre-build/ layout leftover"))
            .collect();
        assert_eq!(warnings.len(), 1, "{lines:?}");
        assert!(
            warnings[0].contains(artifacts::MIGRATION_RECIPE_DOC),
            "{warnings:?}"
        );
        assert!(warnings[0].contains("frust clean"), "{warnings:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// The `--features` passthrough reaches `cargo ndk`: the extras land in
    /// the SAME base64 `-Pfrust.cargoFeatures` CSV as the mode's own features,
    /// appended after them. The gradlew fixture is registered with the exact,
    /// fully-decoded property value, so any other CSV (a replaced selection, a
    /// different order, a second property) finds no registration and fails the
    /// build outright rather than passing on a prefix match.
    #[test]
    fn passthrough_features_ride_the_cargo_features_csv_after_the_modes_own() {
        let dir = unique_project_dir("passthrough-apk");
        let out_dir = dir.join("build/android/app/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake-apk-bytes").unwrap();

        let runner = preflight_ok_runner().with(
            // base64("frust/perf-trace,frust/devtools,hybrid-tier")
            gradlew_key(
                &dir,
                "assembleDebug -Pfrust.targetPlatforms=arm64-v8a \
-Pfrust.splitPerAbi=false \
-Pfrust.cargoFeatures=ZnJ1c3QvcGVyZi10cmFjZSxmcnVzdC9kZXZ0b29scyxoeWJyaWQtdGllcg==",
            ),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Debug, None);
        let result = build_with_env(
            &runner,
            &dir,
            &build_info,
            &target,
            &["hybrid-tier".to_string()],
            &fake_env(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-debug.apk")]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_leftover_artifact_errors_instead_of_being_reported_as_built() {
        // AGP writes every APK shape into the same output directory, so a
        // leftover from a previous, differently-shaped build (e.g. a stale
        // split APK) sitting next to the fresh fat APK this build produced
        // must be caught as an error, never silently printed as "Built"
        // alongside/instead of the real artifact.
        let dir = unique_project_dir("stale-leftover");
        let out_dir = dir.join("build/android/app/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fresh-apk-bytes").unwrap();
        fs::write(
            out_dir.join("app-arm64-v8a-debug.apk"),
            b"stale-leftover-from-a-prior-split-build",
        )
        .unwrap();

        let runner = preflight_ok_runner().with(
            gradlew_key(
                &dir,
                "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            ),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Debug, None);
        let err = build_with_env(
            &runner,
            &dir,
            &build_info,
            &target,
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("expected 1"), "{message}");
        assert!(message.contains("found 2"), "{message}");
        assert!(message.contains("frust clean"), "{message}");
        assert!(
            !message.contains("Built"),
            "error must not itself echo a stale-path 'Built' report: {message}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_apk_with_flavor_and_defines_asserts_exact_argv() {
        let dir = unique_project_dir("full-release-flavor-defines");
        let android_dir = dir.join("android");
        write_release_signing(&android_dir);
        let out_dir = dir.join("build/android/app/outputs/apk/paid/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-paid-release.apk"), b"fake").unwrap();

        let runner = preflight_ok_runner().with(
            gradlew_key(
                &dir,
                "assemblePaidRelease -Pfrust.targetPlatforms=arm64-v8a,x86_64 \
-Pfrust.splitPerAbi=false -Pfrust.defines=QT0xO0I9Mg==",
            ),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string(), "x86_64".to_string()],
        };
        let build_args = BuildArgs {
            flavor: Some("paid".to_string()),
            release: true,
            defines: vec!["A=1".to_string(), "B=2".to_string()],
            ..BuildArgs::default()
        };
        let build_info = BuildInfo::from_args(build_args, BuildMode::Release).unwrap();

        let result = build_with_env(
            &runner,
            &dir,
            &build_info,
            &target,
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-paid-release.apk")]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_without_key_properties_errs_before_invoking_gradle() {
        let dir = unique_project_dir("release-no-keystore");
        // No fixture registered for `./gradlew ...` at all: an unexpected
        // call would itself error, proving Gradle is never invoked.
        let runner = preflight_ok_runner();

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Release, None);
        let err = build_with_env(
            &runner,
            &dir,
            &build_info,
            &target,
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("keytool"), "{err}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// The line the current generated template prints when its release build
    /// falls through to the debug signing config.
    const FALLBACK_WARNING: &str = "Frust: release build is debug-signed. \
[FRUST-SIGNING-FALLBACK] Build with `frust build apk --release` …";

    /// The same line as an installed-base project generated *before* the
    /// machine-stable token existed prints it — the pre-token route, where the
    /// project's Gradle has no `frustSigning(...)` read at all.
    const FALLBACK_WARNING_LEGACY: &str =
        "Frust: release build is debug-signed; create android/key.properties …";

    /// Registers a successful `assembleRelease` whose output carries `warning`,
    /// against a release-signed project with a real artifact on disk — so
    /// nothing *but* the marker can fail the build.
    fn release_runner_emitting(project_dir: &Path, warning: &str) -> FakeProcessRunner {
        preflight_ok_runner().with(
            gradlew_key(
                project_dir,
                "assembleRelease -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            ),
            Output {
                success: true,
                stdout: format!("> Task :app:assembleRelease\n{warning}\nBUILD SUCCESSFUL"),
                stderr: String::new(),
            },
        )
    }

    fn arm64_apk() -> AndroidArtifact {
        AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        }
    }

    /// Plants the release APK `artifacts::discover` would find, so a build that
    /// wrongly *passes* the marker check reports success rather than tripping
    /// over a missing artifact — the failure has to come from the marker.
    fn plant_release_apk(project_dir: &Path) -> PathBuf {
        let out_dir = project_dir.join("build/android/app/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        let apk = out_dir.join("app-release.apk");
        fs::write(&apk, b"fake").unwrap();
        apk
    }

    /// **The backstop.** The gate resolved material and wrote
    /// `.frust-signing.properties`, Gradle exited 0 — and said it debug-signed
    /// anyway (the project's `build.gradle.kts` never read the file). Success
    /// plus a debug-signed artifact is the whole defect class; it must be a
    /// hard failure, not a `BuiltArtifacts`.
    #[test]
    fn release_bails_when_gradle_reports_it_debug_signed() {
        let dir = unique_project_dir("gradle-debug-signed");
        let android_dir = dir.join("android");
        write_release_signing(&android_dir);
        plant_release_apk(&dir);

        let runner = release_runner_emitting(&dir, FALLBACK_WARNING);
        let build_info = info(BuildMode::Release, None);
        let err = build_with_env(
            &runner,
            &dir,
            &build_info,
            &arm64_apk(),
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("Gradle debug-signed"), "{message}");
        assert!(message.contains("build.gradle.kts"), "{message}");
        assert!(message.contains("external = true"), "{message}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// **The pre-token route.** An installed-base project prints the *old* prose, with
    /// no `FRUST-SIGNING-FALLBACK` token in it — the matcher accepts both, so
    /// the projects that most need this backstop are the ones it covers.
    #[test]
    fn release_bails_on_the_pre_token_warning_an_old_template_prints() {
        let dir = unique_project_dir("gradle-debug-signed-legacy");
        let android_dir = dir.join("android");
        write_release_signing(&android_dir);
        plant_release_apk(&dir);

        let runner = release_runner_emitting(&dir, FALLBACK_WARNING_LEGACY);
        let build_info = info(BuildMode::Release, None);
        let err = build_with_env(
            &runner,
            &dir,
            &build_info,
            &arm64_apk(),
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("Gradle debug-signed"), "{err}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// `[signing] external = true` is a declared, warned-about bypass: Frust
    /// promises nothing about the signature, so the marker must not start hard
    /// failing that path. The build succeeds and the waiver warning still
    /// fires.
    #[test]
    fn external_signing_still_passes_when_gradle_reports_debug_signing() {
        let dir = unique_project_dir("external-debug-signed");
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[signing]\nexternal = true\n",
        )
        .unwrap();
        let apk = plant_release_apk(&dir);

        let runner = release_runner_emitting(&dir, FALLBACK_WARNING);
        let build_info = info(BuildMode::Release, None);
        let mut lines = Vec::new();
        let result = build_with_env(
            &runner,
            &dir,
            &build_info,
            &arm64_apk(),
            NO_EXTRA,
            &fake_env(),
            &mut |l| lines.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(result.paths, vec![apk]);
        assert!(
            lines.iter().any(|l| l.contains("external = true")),
            "the waiver must still announce itself: {lines:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// A debug build is debug-signed by design. The template's release-only
    /// warning cannot reach a debug task, but the check is gated on the gate
    /// having promised something regardless — so a stray marker in debug output
    /// never fails a build Frust made no signing promise about.
    #[test]
    fn a_debug_build_is_not_failed_by_the_marker() {
        let dir = unique_project_dir("debug-marker");
        let out_dir = dir.join("build/android/app/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();

        let runner = preflight_ok_runner().with(
            gradlew_key(
                &dir,
                "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            ),
            Output {
                success: true,
                stdout: format!("{FALLBACK_WARNING}\nBUILD SUCCESSFUL"),
                stderr: String::new(),
            },
        );
        let build_info = info(BuildMode::Debug, None);
        let result = build_with_env(
            &runner,
            &dir,
            &build_info,
            &arm64_apk(),
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-debug.apk")]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_flavor_gradle_failure_is_reexplained() {
        let dir = unique_project_dir("missing-flavor");
        let runner = preflight_ok_runner().with(
            gradlew_key(
                &dir,
                "assemblePaidRelease -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            ),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "FAILURE: Build failed with an exception.\n\n\
* What went wrong:\nTask 'assemblePaidRelease' not found in root project 'myapp'."
                    .to_string(),
            },
        );
        write_release_signing(&dir.join("android"));

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Release, Some("paid"));
        let err = build_with_env(
            &runner,
            &dir,
            &build_info,
            &target,
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("flavor 'paid'"), "{message}");
        assert!(message.contains("build.gradle.kts"), "{message}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_gradle_failure_passes_through_stderr_tail() {
        let dir = unique_project_dir("other-failure");
        let runner = preflight_ok_runner().with(
            gradlew_key(
                &dir,
                "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            ),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "e: compile error in MainActivity.kt".to_string(),
            },
        );

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Debug, None);
        let err = build_with_env(
            &runner,
            &dir,
            &build_info,
            &target,
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("compile error in MainActivity.kt"),
            "{err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Legacy direction: a `--release` build against an app whose Cargo.toml
    /// declares no `lean` feature drops it and warns once through `on_line`,
    /// and the Gradle invocation carries NO `-Pfrust.cargoFeatures` prop —
    /// never an undeclared `--features lean` cargo would reject. The
    /// gradlew fixture is registered WITHOUT the cargoFeatures prop, so a
    /// regression that kept `lean` would surface via the absent warning.
    #[test]
    fn release_legacy_app_drops_lean_and_warns() {
        let dir = unique_project_dir("f2-legacy");
        let android_dir = dir.join("android");
        write_release_signing(&android_dir);
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
        let out_dir = dir.join("build/android/app/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();

        let runner = preflight_ok_runner().with(
            gradlew_key(
                &dir,
                "assembleRelease -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            ),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Release, None);
        let mut lines = Vec::new();
        let result = build_with_env(
            &runner,
            &dir,
            &build_info,
            &target,
            NO_EXTRA,
            &fake_env(),
            &mut |l| lines.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-release.apk")]);
        assert!(
            lines.iter().any(|l| l.contains("lean")),
            "legacy release build must warn about the missing `lean` feature: {lines:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Declaring direction: an app that declares `lean` keeps it —
    /// byte-identical `-Pfrust.cargoFeatures=bGVhbg==` (base64 "lean") — and
    /// warns nothing. The gradlew fixture is registered WITH the cargoFeatures
    /// prop; a regression that dropped `lean` would produce a shorter argv that
    /// fails to match, erroring the build.
    #[test]
    fn release_declaring_app_keeps_lean_without_warning() {
        let dir = unique_project_dir("f2-declaring");
        let android_dir = dir.join("android");
        write_release_signing(&android_dir);
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\n\n[features]\nlean = [\"log/release_max_level_warn\"]\n",
        )
        .unwrap();
        let out_dir = dir.join("build/android/app/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();

        let runner = preflight_ok_runner().with(
            gradlew_key(
                &dir,
                "assembleRelease -Pfrust.targetPlatforms=arm64-v8a \
-Pfrust.splitPerAbi=false -Pfrust.cargoFeatures=bGVhbg==",
            ),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Release, None);
        let mut lines = Vec::new();
        let result = build_with_env(
            &runner,
            &dir,
            &build_info,
            &target,
            NO_EXTRA,
            &fake_env(),
            &mut |l| lines.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-release.apk")]);
        assert!(
            !lines.iter().any(|l| l.contains("lean")),
            "a declaring app must not warn: {lines:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn appbundle_build_asserts_bundle_task_and_all_abis() {
        let dir = unique_project_dir("appbundle");
        let android_dir = dir.join("android");
        let out_dir = dir.join("build/android/app/outputs/bundle/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.aab"), b"fake").unwrap();

        let runner = preflight_ok_runner().with(
            gradlew_key(
                &dir,
                "bundleRelease -Pfrust.targetPlatforms=arm64-v8a,armeabi-v7a,x86_64",
            ),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let build_info = info(BuildMode::Release, None);
        write_release_signing(&android_dir);
        let result = build_with_env(
            &runner,
            &dir,
            &build_info,
            &AndroidArtifact::Appbundle,
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-release.aab")]);

        let _ = fs::remove_dir_all(&dir);
    }
}
