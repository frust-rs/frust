//! Android release build pipeline: preflight (reusing `android_run`'s
//! checks, minus the device-only `adb` one) → resolve the app identity
//! (`crate::identity`), route the flavor and merge-write the identity with
//! the version into `android/local.properties` ([`prepare_identity`]) → gate
//! release builds on a keystore and hand the resolved material to Gradle as
//! a short-lived `android/.frust-signing.properties` → compute
//! the `assemble<Flavor><Mode>`/`bundle<Flavor><Mode>` Gradle task (see
//! [`tasks::FlavorRoute`]) and its `-P` properties → run `./gradlew` →
//! glob-verify and report the produced artifact(s) → check what frust.toml
//! states against the built APK ([`check_built_identity`]).

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

use crate::android_run::badging::LaunchIdentity;
use crate::android_run::preflight::{self, PreflightCtx};
use crate::build_info::{BuildInfo, BuildMode, resolve_version};
use crate::doctor::{EnvLookup, RealEnv};
use crate::facade_key::FacadeKey;
use crate::identity::{self, AppIdentity, IdentityPlatform};
use crate::manifest::Manifest;
use crate::process::{ProcessRunner, tail_lines};
use tasks::FlavorRoute;

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
/// Every canonical `frust/<feature>` in that CSV is spelled with the app's
/// facade dependency key ([`FacadeKey`], resolved once from `project_dir`),
/// since `cargo ndk` resolves it against the key; a manifest naming the
/// facade under two keys is an error before Gradle runs.
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

    let manifest = crate::manifest::load_optional(project_dir)?;
    let resolved = resolve_version(info, manifest.as_ref());
    let (version_name, version_code) = resolved.android_name_and_code();
    if info.mode == BuildMode::Release && resolved.is_defaulted() {
        on_line(&resolved.android_unset_warning());
    }
    // The identity rides `local.properties` (each value set when frust.toml
    // states it, removed otherwise) and decides whether a `--flavor` is a
    // Gradle product flavor.
    let app_identity = resolve_identity(manifest.as_ref(), info.flavor.as_deref())?;
    let route = prepare_identity(
        &android_dir,
        info.flavor.as_deref(),
        app_identity.as_ref(),
        &version_name,
        &version_code,
    )?;

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
    // The CSV is handed verbatim to `cargo ndk --features`, so each canonical
    // `frust/<feature>` is spelled with the app's own facade dependency key
    // here, the one boundary the key-agnostic Gradle template trusts.
    let key = FacadeKey::resolve(project_dir).context("resolving the app's frust dependency")?;
    let features: Vec<String> = features.iter().map(|feature| key.map(feature)).collect();

    // Resolve the Debug/Profile feature crates in this process, so a stale
    // index entry surfaces as cargo's own text rather than from inside
    // Gradle. Release adds no crate, so it skips the resolve.
    if info.mode != BuildMode::Release {
        crate::lock_refresh::ensure_resolves(runner, project_dir, &features, on_line)
            .map_err(|err| anyhow::anyhow!(err))?;
    }

    let task = tasks::task_name(target, info.mode, route.gradle_flavor());
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

    // `gradlew.bat` on Windows, absolute — `./gradlew` (relative, run in
    // `android_dir`) unchanged elsewhere; see
    // `android_run::gradle::gradle_wrapper`'s doc for why. Shared with
    // `android_run::gradle::assemble` so the two Gradle-invoking lanes spawn
    // the identical program.
    let wrapper = crate::android_run::gradle::gradle_wrapper(&android_dir);
    let wrapper_cmd = wrapper.to_string_lossy().into_owned();
    let display = crate::android_run::gradle::gradle_wrapper_display();

    let mut prefixed = |line: &str| on_line(&format!("[gradle] {line}"));
    let out = runner
        .run_streaming(
            &wrapper_cmd,
            &args,
            Some(&android_dir),
            &[("JAVA_HOME", &outcome.java_home)],
            &mut prefixed,
        )
        .with_context(|| format!("running `{display} {task}` in `{}`", android_dir.display()))?;
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
                 android/app/build.gradle.kts (see the commented example), or declare \
                 `[flavors.{flavor}]` in frust.toml"
            );
        }
        let tail = tail_lines(&out.stderr, 50);
        let hint = crate::desktop_run::facade_feature_hint(info.mode, &combined)
            .map(|hint| format!("\n{hint}"))
            .unwrap_or_default();
        if tail.is_empty() {
            bail!("`./gradlew {task}` failed{hint}");
        }
        bail!("`./gradlew {task}` failed:\n{tail}{hint}");
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
        route.gradle_flavor(),
        on_line,
    )?;
    for path in &paths {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        on_line(&format!("{} ({size} bytes)", path.display()));
    }

    if let Some(resolved) = app_identity.as_ref() {
        let expected = &resolved.expected;
        match (target, paths.first()) {
            (AndroidArtifact::Apk { .. }, Some(apk)) if checks_anything(expected, &route) => {
                let apk = apk.to_string_lossy();
                let badging = crate::android_run::badging::read(runner, env, &apk);
                check_built_identity(
                    badging.as_ref().map_err(String::as_str),
                    resolved,
                    &route,
                    on_line,
                )?;
            }
            (AndroidArtifact::Apk { .. }, _) => {}
            (AndroidArtifact::Appbundle, _) if expected.identifier_stated => on_line(&format!(
                "note: the application id of an .aab is not verified (frust.toml resolves `{}`).",
                expected.identifier
            )),
            (AndroidArtifact::Appbundle, _) => {}
        }
    }

    Ok(BuiltArtifacts { paths })
}

/// The Android identity a build of a flavor must produce, with the template
/// defaults its migration guidance names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedIdentity {
    pub expected: AppIdentity,
    pub guidance: Guidance,
}

/// The values a hand-run Gradle build of the generated template falls back
/// to — the no-flavor identifier and the default display name — which the
/// migration guidance lines carry as `getProperty` defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Guidance {
    pub identifier: String,
    pub label: String,
}

impl Guidance {
    /// The lines of the generated templates that carry the resolved identity
    /// into a build, with this guidance's identifier and label as the
    /// defaults — what an older project adds to adopt it.
    pub(crate) fn lines(&self) -> Vec<String> {
        vec![
            "Add to android/app/build.gradle.kts, beside the frust.versionName read:".to_string(),
            format!(
                "    val applicationIdValue = localProperties.getProperty(\"{}\", \"{}\")",
                local_properties::APPLICATION_ID_KEY,
                self.identifier
            ),
            format!(
                "    val appLabelValue = localProperties.getProperty(\"{}\", \"{}\")",
                local_properties::APP_LABEL_KEY,
                self.label
            ),
            "and in its defaultConfig block:".to_string(),
            "        applicationId = applicationIdValue".to_string(),
            "        manifestPlaceholders[\"frustAppLabel\"] = appLabelValue".to_string(),
            "and on <application> in android/app/src/main/AndroidManifest.xml:".to_string(),
            "        android:label=\"${frustAppLabel}\"".to_string(),
        ]
    }
}

/// The Android identity a build of `flavor` must produce; `None` when the
/// project has no manifest.
pub(crate) fn resolve_identity(
    manifest: Option<&Manifest>,
    flavor: Option<&str>,
) -> Result<Option<ResolvedIdentity>> {
    manifest
        .map(|manifest| -> Result<_, identity::IdentityError> {
            Ok(ResolvedIdentity {
                expected: identity::resolve(manifest, IdentityPlatform::Android, flavor)?,
                guidance: Guidance {
                    identifier: identity::resolve(manifest, IdentityPlatform::Android, None)?
                        .identifier,
                    label: identity::default_display_name(manifest),
                },
            })
        })
        .transpose()
        .context("resolving the Android application id from frust.toml")
}

/// The pre-Gradle half both Android lanes share: routes `flavor` (refusing
/// one declared both as a product flavor and in `[flavors]`), checks that
/// `android/app/build.gradle.kts` reads every identity value about to be
/// written ([`check_script_reads_identity`]), then merge-writes the version
/// and identity into `local.properties`. Nothing is written on an error.
pub(crate) fn prepare_identity(
    android_dir: &Path,
    flavor: Option<&str>,
    resolved: Option<&ResolvedIdentity>,
    version_name: &str,
    version_code: &str,
) -> Result<FlavorRoute> {
    let script = tasks::read_app_script(android_dir);
    let route = FlavorRoute::select(
        script.as_deref(),
        flavor,
        resolved.is_some_and(|resolved| resolved.expected.flavor_declared),
    )?;
    if let Some(resolved) = resolved {
        check_script_reads_identity(script.as_deref(), resolved)?;
    }
    local_properties::write(
        android_dir,
        version_name,
        version_code,
        resolved.map(|resolved| &resolved.expected),
    )
    .context("writing android/local.properties")?;
    Ok(route)
}

/// Fails before Gradle runs when the app module's build script (`script`,
/// `None` when unreadable, which leaves the post-build check to report) does
/// not read `frust.applicationId` while frust.toml states the identifier, or
/// `frust.appLabel` while it states a display name: Gradle would ignore the
/// value, so the build would carry the project's literal instead.
fn check_script_reads_identity(script: Option<&str>, resolved: &ResolvedIdentity) -> Result<()> {
    let Some(script) = script else {
        return Ok(());
    };
    let expected = &resolved.expected;
    let missing = if expected.identifier_stated
        && !tasks::reads_property(script, local_properties::APPLICATION_ID_KEY)
    {
        Some((
            "application id",
            expected.identifier.as_str(),
            local_properties::APPLICATION_ID_KEY,
        ))
    } else {
        expected
            .display_name
            .as_deref()
            .filter(|_| !tasks::reads_property(script, local_properties::APP_LABEL_KEY))
            .map(|name| ("app label", name, local_properties::APP_LABEL_KEY))
    };
    if let Some((what, value, key)) = missing {
        bail!(
            "frust.toml resolves the {what} `{value}`, but android/app/build.gradle.kts does not \
             read `{key}`.\n{}",
            resolved.guidance.lines().join("\n")
        );
    }
    Ok(())
}

/// Whether [`check_built_identity`] would compare anything for `expected`
/// built through `route`: a stated identifier (compared, or noted for a
/// native flavor), or a display name on a non-native route.
fn checks_anything(expected: &AppIdentity, route: &FlavorRoute) -> bool {
    expected.identifier_stated || (expected.display_name.is_some() && !route.is_native())
}

/// The post-build identity check both Android lanes run on the APK they
/// built, for what frust.toml states only (an unstated value writes no
/// override, so the project's own value is right). A stated identifier other
/// than the built one is an error naming both, with the template lines that
/// let `frust.applicationId` reach it — only a note when `route` built a
/// native Gradle flavor, which may add its own `applicationIdSuffix`. A
/// stated display name other than the built label is a `warning:` with the
/// same guidance, not judged for a native flavor (it owns its label too).
/// Unreadable `badging` (the error says why) skips the check with a note.
pub(crate) fn check_built_identity(
    badging: Result<&LaunchIdentity, &str>,
    resolved: &ResolvedIdentity,
    route: &FlavorRoute,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    let expected = &resolved.expected;
    let label = expected
        .display_name
        .as_deref()
        .filter(|_| !route.is_native());
    if !expected.identifier_stated && label.is_none() {
        return Ok(());
    }
    let built = match badging {
        Ok(built) => built,
        Err(reason) => {
            let what = if expected.identifier_stated {
                "application id"
            } else {
                "label"
            };
            on_line(&format!(
                "note: the built APK's {what} was not verified: {reason}."
            ));
            return Ok(());
        }
    };
    if expected.identifier_stated && route.is_native() {
        on_line(&format!(
            "note: the built APK's application id `{}` was not compared with frust.toml's `{}`: \
             the native Gradle product flavor may add its own `applicationIdSuffix`.",
            built.package, expected.identifier
        ));
    } else if expected.identifier_stated && built.package != expected.identifier {
        bail!(
            "the built APK's application id is `{}`, but frust.toml resolves `{}`: \
             android/app/build.gradle.kts does not read `frust.applicationId`.\n{}",
            built.package,
            expected.identifier,
            resolved.guidance.lines().join("\n")
        );
    }
    if let (Some(expected_label), Some(built_label)) = (label, built.label.as_deref())
        && built_label != expected_label
    {
        on_line(&format!(
            "warning: the built APK's label is `{built_label}`, but frust.toml resolves \
             `{expected_label}`: the project does not read `frust.appLabel`."
        ));
        for line in resolved.guidance.lines() {
            on_line(&line);
        }
    }
    Ok(())
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
    ///
    /// The leading program name is [`crate::android_run::gradle::gradle_wrapper`]'s
    /// own output — `./gradlew` everywhere but Windows, where
    /// `build_with_env` spawns the absolute `gradlew.bat` wrapper — rather
    /// than a hardcoded `"./gradlew"`, so this fixture key matches what the
    /// pipeline actually invokes on every host `cargo test` runs on.
    fn gradlew_key(project_dir: &Path, task_and_props: &str) -> String {
        let android_dir = project_dir.join("android");
        format!(
            "{} --project-cache-dir {} {task_and_props}",
            crate::android_run::gradle::gradle_wrapper(&android_dir).to_string_lossy(),
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
                "cargo metadata --format-version 1",
                Output {
                    success: true,
                    stdout: "{}".to_string(),
                    stderr: String::new(),
                },
            )
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

    /// Runs an Android build in a project with `frust.toml` body `toml`
    /// (`None` writes no manifest) and returns the `local.properties` text and
    /// every `on_line` line.
    fn version_build(
        tag: &str,
        toml: Option<&str>,
        build_info: &BuildInfo,
        target: &AndroidArtifact,
        output_subdir: &str,
        artifact: &str,
        gradle_args: &str,
    ) -> (String, Vec<String>) {
        let dir = unique_project_dir(tag);
        if let Some(toml) = toml {
            fs::write(dir.join("frust.toml"), toml).unwrap();
        }
        if build_info.mode == BuildMode::Release {
            write_release_signing(&dir.join("android"));
        }
        let out_dir = dir.join(output_subdir);
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join(artifact), b"fake").unwrap();
        let runner = preflight_ok_runner().with(
            gradlew_key(&dir, gradle_args),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );
        let mut lines = Vec::new();
        build_with_env(
            &runner,
            &dir,
            build_info,
            target,
            NO_EXTRA,
            &fake_env(),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        let props = fs::read_to_string(dir.join("android/local.properties")).unwrap();
        let _ = fs::remove_dir_all(&dir);
        (props, lines)
    }

    const APK_RELEASE_ARGS: &str =
        "assembleRelease -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false";
    const AAB_RELEASE_ARGS: &str = "bundleRelease -Pfrust.targetPlatforms=arm64-v8a";
    const MANIFEST_VERSION: &str =
        "[app]\nname = \"a\"\norg = \"o\"\nversion = \"3.4.5\"\nbuild-number = 12\n";
    const MANIFEST_NO_VERSION: &str = "[app]\nname = \"a\"\norg = \"o\"\n";

    fn version_warnings(lines: &[String]) -> Vec<&String> {
        lines
            .iter()
            .filter(|l| l.starts_with("warning: app version not set"))
            .collect()
    }

    #[test]
    fn manifest_version_reaches_local_properties_without_a_warning() {
        let (props, lines) = version_build(
            "ver-manifest",
            Some(MANIFEST_VERSION),
            &info(BuildMode::Release, None),
            &arm64_apk(),
            "build/android/app/outputs/apk/release",
            "app-release.apk",
            APK_RELEASE_ARGS,
        );
        assert!(props.contains("frust.versionName=3.4.5"), "{props}");
        assert!(props.contains("frust.versionCode=12"), "{props}");
        assert!(version_warnings(&lines).is_empty(), "{lines:?}");
    }

    #[test]
    fn flag_version_beats_the_manifest_in_local_properties() {
        let flagged = BuildInfo::from_args(
            BuildArgs {
                build_name: Some("9.9.9".to_string()),
                build_number: Some(99),
                ..BuildArgs::default()
            },
            BuildMode::Release,
        )
        .unwrap();
        let (props, lines) = version_build(
            "ver-flag",
            Some(MANIFEST_VERSION),
            &flagged,
            &arm64_apk(),
            "build/android/app/outputs/apk/release",
            "app-release.apk",
            APK_RELEASE_ARGS,
        );
        assert!(props.contains("frust.versionName=9.9.9"), "{props}");
        assert!(props.contains("frust.versionCode=99"), "{props}");
        assert!(version_warnings(&lines).is_empty(), "{lines:?}");
    }

    #[test]
    fn release_apk_and_appbundle_with_defaults_warn_exactly_once() {
        let warning = crate::build_info::resolve_version(&info(BuildMode::Release, None), None)
            .android_unset_warning();
        let warning = warning.as_str();
        for (manifest, label) in [(None, "none"), (Some(MANIFEST_NO_VERSION), "empty")] {
            let (props, lines) = version_build(
                &format!("ver-default-apk-{label}"),
                manifest,
                &info(BuildMode::Release, None),
                &arm64_apk(),
                "build/android/app/outputs/apk/release",
                "app-release.apk",
                APK_RELEASE_ARGS,
            );
            assert!(props.contains("frust.versionName=1.0"), "{props}");
            assert_eq!(version_warnings(&lines), vec![warning], "{lines:?}");
        }
        let (_, lines) = version_build(
            "ver-default-aab",
            Some(MANIFEST_NO_VERSION),
            &info(BuildMode::Release, None),
            &AndroidArtifact::Appbundle,
            "build/android/app/outputs/bundle/release",
            "app-release.aab",
            AAB_RELEASE_ARGS,
        );
        assert_eq!(version_warnings(&lines), vec![warning], "{lines:?}");
    }

    #[test]
    fn a_partly_defaulted_release_build_warns_with_the_values_used() {
        let (_, lines) = version_build(
            "ver-partial",
            Some("[app]\nname = \"a\"\norg = \"o\"\nversion = \"3.4.5\"\n"),
            &info(BuildMode::Release, None),
            &arm64_apk(),
            "build/android/app/outputs/apk/release",
            "app-release.apk",
            APK_RELEASE_ARGS,
        );
        let warnings = version_warnings(&lines);
        assert_eq!(warnings.len(), 1, "{lines:?}");
        assert!(warnings[0].contains("building 3.4.5 (1)"), "{warnings:?}");
    }

    #[test]
    fn a_debug_build_with_defaults_does_not_warn() {
        let (_, lines) = version_build(
            "ver-debug",
            Some(MANIFEST_NO_VERSION),
            &info(BuildMode::Debug, None),
            &arm64_apk(),
            "build/android/app/outputs/apk/debug",
            "app-debug.apk",
            "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
        );
        assert!(version_warnings(&lines).is_empty(), "{lines:?}");
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
    /// `android/app/src/main/jniLibs` on a MIGRATED project (its own
    /// `android/app/build.gradle.kts` carries the r1-04 `setSrcDirs`
    /// redirect, so this directory is never packaged into anything Gradle
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
        fs::create_dir_all(dir.join("android/app")).unwrap();
        fs::write(
            dir.join("android/app/build.gradle.kts"),
            format!(
                "sourceSets.getByName(\"main\").jniLibs.setSrcDirs(listOf(\"../../{}\"))\n",
                crate::build_dirs::BuildLayout::android_jni_libs().display()
            ),
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

    /// An app keyed `frust-ui`: the mode's facade features reach the CSV as
    /// `frust-ui/<feature>`, the passthrough after them unchanged — decoded,
    /// `frust-ui/perf-trace,frust-ui/devtools,hybrid-tier`.
    #[test]
    fn a_frust_ui_keyed_app_spells_the_cargo_features_csv_with_its_key() {
        let dir = unique_project_dir("frust-ui-key");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfrust-ui = \"0.6.0\"\n",
        )
        .unwrap();
        let out_dir = dir.join("build/android/app/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake-apk-bytes").unwrap();

        let runner = preflight_ok_runner().with(
            // base64("frust-ui/perf-trace,frust-ui/devtools,hybrid-tier")
            gradlew_key(
                &dir,
                "assembleDebug -Pfrust.targetPlatforms=arm64-v8a \
-Pfrust.splitPerAbi=false \
-Pfrust.cargoFeatures=ZnJ1c3QtdWkvcGVyZi10cmFjZSxmcnVzdC11aS9kZXZ0b29scyxoeWJyaWQtdGllcg==",
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
        let result = build_with_env(
            &runner,
            &dir,
            &info(BuildMode::Debug, None),
            &target,
            &["hybrid-tier".to_string()],
            &fake_env(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-debug.apk")]);

        let _ = fs::remove_dir_all(&dir);
    }

    /// A Gradle failure whose cargo output names a feature the package does
    /// not contain carries the dependency-key hint.
    #[test]
    fn a_missing_feature_failure_carries_the_facade_key_hint() {
        let dir = unique_project_dir("feature-hint");
        let runner = preflight_ok_runner().with(
            gradlew_key(&dir, "assembleDebug"),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error: the package 'app' does not contain this feature: frust/devtools"
                    .to_string(),
            },
        );
        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let err = build_with_env(
            &runner,
            &dir,
            &info(BuildMode::Debug, None),
            &target,
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.ends_with(FacadeKey::hint()), "{message}");

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

    /// A Debug build resolves the app's dependency graph first, under exactly
    /// the mapped features Gradle will pass, and a failure stops the build
    /// with cargo's own text before Gradle is invoked (a Gradle reply is
    /// scripted, so reaching it would succeed).
    #[test]
    fn a_debug_build_resolves_the_graph_first_with_the_mapped_features() {
        let dir = unique_project_dir("resolve-first");
        let out_dir = dir.join("build/android/app/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();

        let runner = preflight_ok_runner()
            .with(
                format!(
                    "cargo metadata --format-version 1 --manifest-path {} --features frust/perf-trace --features frust/devtools",
                    dir.join("Cargo.toml").display()
                ),
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "error: the graph does not resolve".to_string(),
                },
            )
            .with(
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
        let err = build_with_env(
            &runner,
            &dir,
            &info(BuildMode::Debug, None),
            &arm64_apk(),
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "error: the graph does not resolve");

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
        assert!(
            message.contains("`[flavors.paid]` in frust.toml"),
            "{message}"
        );

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

    /// A resolved identity stating `identifier` and, when given, a display
    /// name, with the guidance defaults `dev.f0x.myapp`/`Myapp`.
    fn app_identity(identifier: &str, display_name: Option<&str>) -> ResolvedIdentity {
        ResolvedIdentity {
            expected: AppIdentity {
                identifier: identifier.to_string(),
                identifier_stated: true,
                display_name: display_name.map(str::to_string),
                flavor: None,
                flavor_declared: false,
            },
            guidance: Guidance {
                identifier: "dev.f0x.myapp".to_string(),
                label: "Myapp".to_string(),
            },
        }
    }

    fn launch(package: &str, label: Option<&str>) -> LaunchIdentity {
        LaunchIdentity {
            package: package.to_string(),
            activity: "dev.f0x.myapp.MainActivity".to_string(),
            label: label.map(str::to_string),
        }
    }

    /// Runs [`check_built_identity`] for `expected` against `built`,
    /// returning its result and every `on_line` line.
    fn check(
        built: Result<&LaunchIdentity, &str>,
        expected: &ResolvedIdentity,
        route: &FlavorRoute,
    ) -> (Result<()>, Vec<String>) {
        let mut lines = Vec::new();
        let result = check_built_identity(built, expected, route, &mut |line| {
            lines.push(line.to_string())
        });
        (result, lines)
    }

    /// Every guidance line is a line of the generated templates, once their
    /// placeholders are rendered with the guidance's own defaults.
    #[test]
    fn identity_guidance_names_the_exact_template_lines() {
        let gradle = include_str!("../../templates/app/android.tmpl/app/build.gradle.kts.tmpl")
            .replace("{{ android_identifier }}", "dev.f0x.myapp")
            .replace("{{ title_case_name }}", "Myapp");
        let manifest =
            include_str!("../../templates/app/android.tmpl/app/src/main/AndroidManifest.xml.tmpl");
        let template_lines: Vec<&str> = gradle
            .lines()
            .chain(manifest.lines())
            .map(str::trim)
            .collect();
        let guidance = app_identity("dev.f0x.myapp", None).guidance.lines();
        let code: Vec<&String> = guidance
            .iter()
            .filter(|line| line.starts_with(' '))
            .collect();
        assert_eq!(code.len(), 5, "{guidance:?}");
        for line in code {
            assert!(
                template_lines.contains(&line.trim()),
                "not in the templates: {line}"
            );
        }
    }

    #[test]
    fn a_matching_apk_passes_silently() {
        let expected = app_identity("com.example.other", Some("Other"));
        let built = launch("com.example.other", Some("Other"));
        let (result, lines) = check(Ok(&built), &expected, &FlavorRoute::Unflavored);
        result.unwrap();
        assert!(lines.is_empty(), "{lines:?}");
    }

    /// No display name stated: the built label is not judged, whatever it
    /// is; nothing stated at all checks nothing, not even the badging.
    #[test]
    fn an_unstated_display_name_is_not_checked() {
        let expected = app_identity("com.example.other", None);
        let built = launch("com.example.other", Some("Anything"));
        let (result, lines) = check(Ok(&built), &expected, &FlavorRoute::Unflavored);
        result.unwrap();
        assert!(lines.is_empty(), "{lines:?}");

        let mut nothing = app_identity("dev.f0x.myapp", None);
        nothing.expected.identifier_stated = false;
        assert!(!checks_anything(
            &nothing.expected,
            &FlavorRoute::Unflavored
        ));
        let (result, lines) = check(Err("unreadable"), &nothing, &FlavorRoute::Unflavored);
        result.unwrap();
        assert!(lines.is_empty(), "{lines:?}");
    }

    /// A native flavor owns its label: a stated name is not compared, and
    /// with no stated identifier nothing is checked at all.
    #[test]
    fn the_label_is_not_judged_for_a_native_flavor() {
        let native = FlavorRoute::Native("dev".into());
        let mut expected = app_identity("dev.f0x.myapp", Some("Myapp"));
        expected.expected.identifier_stated = false;
        assert!(!checks_anything(&expected.expected, &native));
        let built = launch("dev.f0x.myapp.dev", Some("Myapp Native"));
        let (result, lines) = check(Ok(&built), &expected, &native);
        result.unwrap();
        assert!(lines.is_empty(), "{lines:?}");

        expected.expected.identifier_stated = true;
        let (result, lines) = check(Ok(&built), &expected, &native);
        result.unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("applicationIdSuffix"), "{lines:?}");
    }

    #[test]
    fn a_package_mismatch_names_both_ids_and_the_template_lines() {
        let expected = app_identity("com.example.other", Some("Other"));
        let built = launch("dev.f0x.myapp", Some("Other"));
        let (result, _) = check(Ok(&built), &expected, &FlavorRoute::Unflavored);
        let message = result.unwrap_err().to_string();
        assert!(message.contains("`dev.f0x.myapp`"), "{message}");
        assert!(message.contains("`com.example.other`"), "{message}");
        assert!(
            message.contains(
                "val applicationIdValue = localProperties.getProperty(\"frust.applicationId\", \"dev.f0x.myapp\")"
            ),
            "{message}"
        );
        assert!(
            message.contains("applicationId = applicationIdValue"),
            "{message}"
        );
        assert!(
            message.contains("android:label=\"${frustAppLabel}\""),
            "{message}"
        );
    }

    /// A native product flavor may add its own suffix: the package is not
    /// compared (one note says so), and neither is the label it owns too.
    #[test]
    fn a_native_flavor_skips_the_package_comparison_with_one_note() {
        let expected = app_identity("com.example.other", Some("Other"));
        let built = launch("com.example.other.dev", Some("Other Native"));
        let (result, lines) = check(Ok(&built), &expected, &FlavorRoute::Native("dev".into()));
        result.unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("note:"), "{lines:?}");
        assert!(lines[0].contains("applicationIdSuffix"), "{lines:?}");
    }

    #[test]
    fn a_label_mismatch_is_a_warning_with_the_guidance() {
        let expected = app_identity("com.example.other", Some("Other Dev"));
        let built = launch("com.example.other", Some("Myapp"));
        let (result, lines) = check(
            Ok(&built),
            &expected,
            &FlavorRoute::ManifestOnly("dev".into()),
        );
        result.unwrap();
        let warnings: Vec<&String> = lines.iter().filter(|l| l.starts_with("warning:")).collect();
        assert_eq!(warnings.len(), 1, "{lines:?}");
        assert!(warnings[0].contains("`Myapp`") && warnings[0].contains("`Other Dev`"));
        assert!(
            lines
                .iter()
                .any(|l| l.trim() == "manifestPlaceholders[\"frustAppLabel\"] = appLabelValue"),
            "{lines:?}"
        );
    }

    #[test]
    fn unreadable_badging_skips_the_check_with_a_note() {
        let expected = app_identity("com.example.other", Some("Other"));
        let (result, lines) = check(
            Err("`aapt2` could not be run (not found)"),
            &expected,
            &FlavorRoute::Unflavored,
        );
        result.unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("note:") && lines[0].contains("aapt2"),
            "{lines:?}"
        );
    }

    /// Plants an SDK with one build-tools `aapt2` under `dir`, returning the
    /// `aapt2` path and the `ANDROID_HOME` value.
    fn plant_aapt2(dir: &Path) -> (String, String) {
        let aapt2_bin = if cfg!(windows) { "aapt2.exe" } else { "aapt2" };
        let build_tools = dir.join("sdk").join("build-tools").join("35.0.1");
        fs::create_dir_all(&build_tools).unwrap();
        let aapt2 = build_tools.join(aapt2_bin);
        fs::write(&aapt2, "#!/bin/sh\n").unwrap();
        (
            aapt2.to_string_lossy().into_owned(),
            dir.join("sdk").to_string_lossy().into_owned(),
        )
    }

    const MANIFEST_DEV_FLAVOR: &str = "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
        [flavors.dev]\napp-id-suffix = \".dev\"\napp-name-suffix = \" Dev\"\n";

    /// A flavor declared only in frust.toml builds the plain variant, writes
    /// the suffixed identity into local.properties, and the built APK is
    /// checked against it — a stale literal `applicationId` fails the build.
    #[test]
    fn a_frust_toml_flavor_builds_the_plain_variant_and_checks_the_apk() {
        for (tag, package, ok) in [
            ("toml-flavor-ok", "dev.f0x.myapp.dev", true),
            ("toml-flavor-stale", "dev.f0x.myapp", false),
        ] {
            let dir = unique_project_dir(tag);
            fs::write(dir.join("frust.toml"), MANIFEST_DEV_FLAVOR).unwrap();
            let out_dir = dir.join("build/android/app/outputs/apk/debug");
            fs::create_dir_all(&out_dir).unwrap();
            let apk = out_dir.join("app-debug.apk");
            fs::write(&apk, b"fake").unwrap();
            let (aapt2, android_home) = plant_aapt2(&dir);
            let runner = preflight_ok_runner()
                .with(
                    gradlew_key(
                        &dir,
                        "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
                    ),
                    Output {
                        success: true,
                        stdout: "BUILD SUCCESSFUL".to_string(),
                        stderr: String::new(),
                    },
                )
                .with(
                    format!("{aapt2} dump badging {}", apk.display()),
                    Output {
                        success: true,
                        stdout: format!(
                            "package: name='{package}'\napplication-label:'Myapp Dev'\n\
                             launchable-activity: name='dev.f0x.myapp.MainActivity'\n"
                        ),
                        stderr: String::new(),
                    },
                );
            let mut lines = Vec::new();
            let result = build_with_env(
                &runner,
                &dir,
                &info(BuildMode::Debug, Some("dev")),
                &arm64_apk(),
                NO_EXTRA,
                &fake_env().set("ANDROID_HOME", &android_home),
                &mut |line| lines.push(line.to_string()),
            );
            let props = fs::read_to_string(dir.join("android/local.properties")).unwrap();
            assert!(
                props.contains("frust.applicationId=dev.f0x.myapp.dev\n"),
                "{props}"
            );
            assert!(props.contains("frust.appLabel=Myapp Dev\n"), "{props}");
            if ok {
                assert_eq!(result.unwrap().paths, vec![apk]);
                assert!(
                    !lines.iter().any(|l| l.starts_with("warning:")),
                    "{lines:?}"
                );
            } else {
                let message = result.unwrap_err().to_string();
                assert!(message.contains("`dev.f0x.myapp.dev`"), "{message}");
            }
            let _ = fs::remove_dir_all(&dir);
        }
    }

    /// An explicit appbundle is not verified — one note says so — and a
    /// later derived build drops the identity keys a flavored build left.
    #[test]
    fn an_explicit_appbundle_notes_it_is_unverified_and_keys_do_not_leak() {
        let dir = unique_project_dir("aab-identity");
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[android]\nidentifier = \"com.example.other\"\n",
        )
        .unwrap();
        write_release_signing(&dir.join("android"));
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
        let run = |lines: &mut Vec<String>| {
            build_with_env(
                &runner,
                &dir,
                &info(BuildMode::Release, None),
                &AndroidArtifact::Appbundle,
                NO_EXTRA,
                &fake_env(),
                &mut |line| lines.push(line.to_string()),
            )
            .unwrap();
        };
        let mut lines = Vec::new();
        run(&mut lines);
        let notes: Vec<&String> = lines.iter().filter(|l| l.starts_with("note:")).collect();
        assert_eq!(notes.len(), 1, "{lines:?}");
        assert!(notes[0].contains(".aab is not verified"), "{notes:?}");
        let props = fs::read_to_string(dir.join("android/local.properties")).unwrap();
        assert!(
            props.contains("frust.applicationId=com.example.other\n"),
            "{props}"
        );

        fs::write(dir.join("frust.toml"), MANIFEST_NO_VERSION).unwrap();
        let mut lines = Vec::new();
        run(&mut lines);
        assert!(!lines.iter().any(|l| l.starts_with("note:")), "{lines:?}");
        let props = fs::read_to_string(dir.join("android/local.properties")).unwrap();
        assert!(!props.contains("frust.applicationId"), "{props}");
        assert!(!props.contains("frust.appLabel"), "{props}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A derived identity with no flavor writes no identity key and runs no
    /// check: no `aapt2` is registered, and no note is sent.
    #[test]
    fn a_derived_identity_builds_exactly_as_before() {
        let (props, lines) = version_build(
            "derived-identity",
            Some(MANIFEST_NO_VERSION),
            &info(BuildMode::Debug, None),
            &arm64_apk(),
            "build/android/app/outputs/apk/debug",
            "app-debug.apk",
            "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
        );
        assert_eq!(props, "frust.versionName=1.0\nfrust.versionCode=1\n");
        assert!(!lines.iter().any(|l| l.starts_with("note:")), "{lines:?}");
    }

    /// A migrated app module script: reads both identity keys and declares
    /// `native` as a product flavor.
    const MIGRATED_SCRIPT: &str = "val applicationIdValue = localProperties.getProperty(\"frust.applicationId\", \"x.y\")\n\
         val appLabelValue = localProperties.getProperty(\"frust.appLabel\", \"Y\")\n\
         android { productFlavors { create(\"native\") { } } }\n";

    /// Runs a debug APK build of `flavor` with no Gradle fixture registered,
    /// so the build can only stop before Gradle: returns the error and
    /// whether `local.properties` was written.
    fn build_stopping_before_gradle(
        tag: &str,
        toml: &str,
        script: &str,
        flavor: Option<&str>,
    ) -> (String, bool) {
        let dir = unique_project_dir(tag);
        fs::write(dir.join("frust.toml"), toml).unwrap();
        fs::create_dir_all(dir.join("android/app")).unwrap();
        fs::write(dir.join("android/app/build.gradle.kts"), script).unwrap();
        let err = build_with_env(
            &preflight_ok_runner(),
            &dir,
            &info(BuildMode::Debug, flavor),
            &arm64_apk(),
            NO_EXTRA,
            &fake_env(),
            &mut |_| {},
        )
        .unwrap_err();
        let written = dir.join("android/local.properties").exists();
        let _ = fs::remove_dir_all(&dir);
        (format!("{err:#}"), written)
    }

    /// A flavor that is both a Gradle product flavor and `[flavors.<name>]`
    /// is refused before Gradle runs (no Gradle fixture is registered) and
    /// before anything is written.
    #[test]
    fn a_double_declared_flavor_fails_before_gradle() {
        let (message, written) = build_stopping_before_gradle(
            "double-declared",
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[flavors.native]\napp-id-suffix = \".n\"\n",
            MIGRATED_SCRIPT,
            Some("native"),
        );
        assert!(
            message.contains("flavor `native` is declared twice"),
            "{message}"
        );
        assert!(
            message.contains("android/app/build.gradle.kts"),
            "{message}"
        );
        assert!(
            message.contains("`[flavors.native]` in frust.toml"),
            "{message}"
        );
        assert!(!written);
    }

    /// An app module that does not read a key frust.toml is about to set
    /// fails in seconds, before Gradle, with the migration guidance.
    #[test]
    fn an_unmigrated_script_fails_before_gradle_for_each_stated_value() {
        let unmigrated = "android { defaultConfig { applicationId = \"dev.f0x.myapp\" } }\n";
        let id_only = "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[android]\nidentifier = \"com.acme.app\"\n";
        let (message, written) =
            build_stopping_before_gradle("unmigrated-id", id_only, unmigrated, None);
        assert!(
            message.contains("resolves the application id `com.acme.app`"),
            "{message}"
        );
        assert!(
            message.contains("does not read `frust.applicationId`"),
            "{message}"
        );
        assert!(
            message.contains("val applicationIdValue = localProperties.getProperty(\"frust.applicationId\", \"com.acme.app\")"),
            "{message}"
        );
        assert!(!written);

        let reads_id_only =
            "val a = localProperties.getProperty(\"frust.applicationId\", \"a.b\")\n";
        let named = "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\ndisplay-name = \"Shop\"\n";
        let (message, _) =
            build_stopping_before_gradle("unmigrated-label", named, reads_id_only, None);
        assert!(
            message.contains("resolves the app label `Shop`"),
            "{message}"
        );
        assert!(
            message.contains("does not read `frust.appLabel`"),
            "{message}"
        );
        assert!(
            message.contains("getProperty(\"frust.appLabel\", \"Shop\")"),
            "{message}"
        );
    }

    /// `display-name` alone writes only the label, which the built APK is
    /// checked against; without it no label is written or judged.
    #[test]
    fn a_stated_display_name_alone_is_written_and_checked() {
        for (tag, toml, label_key) in [
            (
                "display-name",
                "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\ndisplay-name = \"Shop\"\n",
                true,
            ),
            (
                "no-display-name",
                "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[android]\nidentifier = \"dev.f0x.myapp\"\n",
                false,
            ),
        ] {
            let dir = unique_project_dir(tag);
            fs::write(dir.join("frust.toml"), toml).unwrap();
            fs::create_dir_all(dir.join("android/app")).unwrap();
            fs::write(dir.join("android/app/build.gradle.kts"), MIGRATED_SCRIPT).unwrap();
            let out_dir = dir.join("build/android/app/outputs/apk/debug");
            fs::create_dir_all(&out_dir).unwrap();
            let apk = out_dir.join("app-debug.apk");
            fs::write(&apk, b"fake").unwrap();
            let (aapt2, android_home) = plant_aapt2(&dir);
            let runner = preflight_ok_runner()
                .with(
                    gradlew_key(
                        &dir,
                        "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
                    ),
                    Output {
                        success: true,
                        stdout: "BUILD SUCCESSFUL".to_string(),
                        stderr: String::new(),
                    },
                )
                .with(
                    format!("{aapt2} dump badging {}", apk.display()),
                    Output {
                        success: true,
                        stdout: "package: name='dev.f0x.myapp'\napplication-label:'Myapp'\n\
                                 launchable-activity: name='dev.f0x.myapp.MainActivity'\n"
                            .to_string(),
                        stderr: String::new(),
                    },
                );
            let mut lines = Vec::new();
            build_with_env(
                &runner,
                &dir,
                &info(BuildMode::Debug, None),
                &arm64_apk(),
                NO_EXTRA,
                &fake_env().set("ANDROID_HOME", &android_home),
                &mut |line| lines.push(line.to_string()),
            )
            .unwrap();
            let props = fs::read_to_string(dir.join("android/local.properties")).unwrap();
            let warned = lines
                .iter()
                .any(|l| l.starts_with("warning: the built APK's label"));
            assert_eq!(
                props.contains("frust.appLabel=Shop\n"),
                label_key,
                "{props}"
            );
            assert_eq!(props.contains("frust.appLabel"), label_key, "{props}");
            assert_eq!(props.contains("frust.applicationId"), !label_key, "{props}");
            assert_eq!(warned, label_key, "{lines:?}");
            let _ = fs::remove_dir_all(&dir);
        }
    }
}
