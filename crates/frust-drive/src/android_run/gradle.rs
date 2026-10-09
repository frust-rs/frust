//! Drives `./gradlew assemble<Flavor><Mode>` in `<app>/android/`, mode/flavor
//! aware. The generated Gradle project's cargo-ndk task does the actual Rust
//! `.so` build — except in a hot session, which stages the library itself
//! and assembles with `-x cargoNdkBuild` ([`assemble_excluding`]); this
//! module only shells out to Gradle and streams its output.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::android_build::AndroidArtifact;
use crate::build_dirs::BuildLayout;
use crate::build_info::BuildMode;
use crate::process::{Output, ProcessRunner};

/// The Gradle CLI option that relocates Gradle's own per-project cache
/// (`.gradle/`), which Gradle otherwise creates next to `settings.gradle.kts`
/// — inside the app's `android/` source tree. `settings.gradle.kts`'
/// `buildDirectory` redirect cannot move it: the cache is written during
/// *configuration*, before any of that runs, so the CLI has to say where it
/// goes.
const PROJECT_CACHE_DIR_FLAG: &str = "--project-cache-dir";

/// The absolute `--project-cache-dir` value for the project rooted at
/// `project_dir`: its [`BuildLayout::android_gradle_cache`] directory, so
/// Gradle's cache joins every other Android build artifact under
/// `<project>/build/android/`.
///
/// Absolute on purpose. Gradle resolves a relative `--project-cache-dir`
/// against its own working directory (`<project>/android/`, not the project
/// root), which would silently put the cache back in the source tree; and
/// `frust clean` deletes the directory by the project-rooted path, so the two
/// sides must name the same place. Falls back to the lexical path, then the
/// unmodified join, if the process has no readable current directory —
/// defensive only, since the caller has already read files under
/// `project_dir`.
pub fn project_cache_dir(project_dir: &Path) -> PathBuf {
    let cache = project_dir.join(BuildLayout::android_gradle_cache());
    std::path::absolute(&cache).unwrap_or(cache)
}

/// The two leading `./gradlew` arguments every Frust-driven invocation
/// carries, ahead of the task name. Returned as owned `String`s because the
/// cache path is computed per project; both `assemble` here and
/// `android_build::build_with_env`'s own `run_streaming` call build their
/// argument vector from this, so the two lanes cannot drift.
pub fn leading_args(project_dir: &Path) -> [String; 2] {
    [
        PROJECT_CACHE_DIR_FLAG.to_string(),
        project_cache_dir(project_dir)
            .to_string_lossy()
            .into_owned(),
    ]
}

/// The Gradle wrapper program to spawn for `android_dir`, host-appropriate:
/// the relative `./gradlew` (run with `android_dir` as the child's
/// `current_dir`, unchanged) everywhere except Windows, where CreateProcess
/// cannot execute a POSIX shell script and Rust's `Command` neither appends
/// `.bat` nor resolves a bare program name against the child's `current_dir`
/// (only `PATH`) — there this names the absolute `gradlew.bat` wrapper the
/// scaffold also ships, so the spawn works regardless of the process's own
/// working directory.
///
/// `pub(crate)`: `android_build::build_with_env` reuses this rather than
/// re-deriving the wrapper program, so the two Gradle-invoking lanes cannot
/// drift on which file they spawn.
pub(crate) fn gradle_wrapper(android_dir: &Path) -> PathBuf {
    gradle_wrapper_for(android_dir, cfg!(target_os = "windows"))
}

/// [`gradle_wrapper`]'s inner logic, taking `windows` explicitly (rather than
/// baking in `cfg!(target_os = "windows")`) so both arms run under `cargo
/// test` on any host.
fn gradle_wrapper_for(android_dir: &Path, windows: bool) -> PathBuf {
    if windows {
        let candidate = android_dir.join("gradlew.bat");
        std::path::absolute(&candidate).unwrap_or(candidate)
    } else {
        PathBuf::from("./gradlew")
    }
}

/// The wrapper name used in log/error text — just the file name, never
/// [`gradle_wrapper`]'s (possibly long, absolute) Windows path, so a failure
/// message stays readable on either host.
pub(crate) fn gradle_wrapper_display() -> &'static str {
    gradle_wrapper_display_for(cfg!(target_os = "windows"))
}

fn gradle_wrapper_display_for(windows: bool) -> &'static str {
    if windows { "gradlew.bat" } else { "./gradlew" }
}

/// Runs `./gradlew --project-cache-dir <dir> <task> [-P...]` (`gradlew.bat`
/// on Windows — see [`gradle_wrapper`]) in `android_dir` — `task` and `props`
/// are `android_build::tasks::task_name`/`gradle_properties`' output, reused
/// rather than duplicated here so the run and build pipelines compute a
/// variant's task name and `-P` properties from exactly one place. Streams
/// each line of output through `on_line` (prefixed `[gradle] `) and exports
/// `JAVA_HOME=java_home` for the child process only.
///
/// `project_dir` is the Frust project root (`android_dir`'s parent); it is
/// taken explicitly rather than derived so a caller that already resolved it
/// — both do — cannot be given an `android_dir` whose parent is something
/// else. See [`leading_args`] for why the cache directory has to be passed on
/// the command line at all.
pub fn assemble(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    android_dir: &Path,
    java_home: &str,
    task: &str,
    props: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    assemble_excluding(
        runner,
        project_dir,
        android_dir,
        java_home,
        task,
        &[],
        props,
        on_line,
    )
}

/// The generated Gradle project's task that builds the Rust `.so` with
/// `cargo ndk` before every build (`build.gradle.kts.tmpl`'s
/// `cargoNdkBuild`, hooked into `preBuild`). A hot session builds and
/// stages the library itself and excludes this task.
pub const CARGO_NDK_BUILD_TASK: &str = "cargoNdkBuild";

/// [`assemble`] with each of `excluded` skipped through Gradle's own `-x
/// <task>`, placed right after `task`:
/// `./gradlew --project-cache-dir <dir> <task> -x <excluded>... [-P...]`.
/// An empty `excluded` is exactly [`assemble`]'s invocation.
#[allow(clippy::too_many_arguments)] // `assemble`'s inputs plus the exclusions
pub fn assemble_excluding(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    android_dir: &Path,
    java_home: &str,
    task: &str,
    excluded: &[&str],
    props: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    let leading = leading_args(project_dir);
    let mut args: Vec<&str> =
        Vec::with_capacity(leading.len() + 1 + 2 * excluded.len() + props.len());
    for arg in &leading {
        args.push(arg.as_str());
    }
    args.push(task);
    for excluded_task in excluded {
        args.push("-x");
        args.push(excluded_task);
    }
    for prop in props {
        args.push(prop.as_str());
    }

    let wrapper = gradle_wrapper(android_dir);
    let wrapper_cmd = wrapper.to_string_lossy().into_owned();
    let display = gradle_wrapper_display();

    let mut prefixed = |line: &str| on_line(&format!("[gradle] {line}"));
    runner
        .run_streaming(
            &wrapper_cmd,
            &args,
            Some(android_dir),
            &[("JAVA_HOME", java_home)],
            &mut prefixed,
        )
        .with_context(|| format!("running `{display} {task}` in `{}`", android_dir.display()))
}

/// Resolves the single APK a non-split `assemble<Flavor><Mode>` build should
/// have produced, reusing `android_build::artifacts`' directory + glob +
/// expected-count discovery rather than guessing AGP's exact output
/// filename or re-checking the count here — the
/// output-naming/count-guard logic lives in exactly one place. `frust
/// run` always builds a single-ABI, non-split APK (it installs on one
/// connected device), so `discover` itself now errors on anything but
/// exactly one file (e.g. a stale split-build artifact left over in the
/// output directory) rather than this function re-deriving the same check.
pub fn apk_output_path(
    project_dir: &Path,
    mode: BuildMode,
    flavor: Option<&str>,
    on_line: &mut dyn FnMut(&str),
) -> Result<PathBuf> {
    let target = AndroidArtifact::Apk {
        split_per_abi: false,
        abis: Vec::new(),
    };
    let mut found =
        crate::android_build::artifacts::discover(project_dir, &target, mode, flavor, on_line)?;
    debug_assert_eq!(
        found.len(),
        1,
        "discover already guards a non-split APK build to exactly one file"
    );
    Ok(found
        .pop()
        .expect("discover errors on anything but exactly one file"))
}

/// Best-effort check for whether Gradle's wrapper distribution is already
/// cached under `<gradle_user_home>/wrapper/dists/` — used only to print a
/// one-time note that the first run downloads Gradle, never to gate the
/// build.
pub fn wrapper_dist_cached(gradle_user_home: &Path) -> bool {
    gradle_user_home
        .join("wrapper")
        .join("dists")
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// The full `./gradlew` argv this module produces, as the
    /// `FakeProcessRunner` keys it. Registering the WHOLE invocation (rather
    /// than a prefix) is what makes these tests assert the argv: the fake
    /// falls back to the longest registered key that is a *prefix* of the
    /// real one, so a missing or misplaced `--project-cache-dir` finds no
    /// fixture at all and fails the call.
    ///
    /// Leads with [`gradle_wrapper`]'s own output — `./gradlew` everywhere
    /// but Windows, where `assemble` spawns the absolute `gradlew.bat`
    /// wrapper — rather than a hardcoded `"./gradlew"`, so this fixture key
    /// matches what `assemble` actually invokes on every host `cargo test`
    /// runs on.
    fn gradlew_argv(project_dir: &Path, task: &str, props: &[&str]) -> String {
        let mut key = format!(
            "{} --project-cache-dir {} {task}",
            gradle_wrapper(&project_dir.join("android")).to_string_lossy(),
            project_cache_dir(project_dir).display()
        );
        for prop in props {
            key.push(' ');
            key.push_str(prop);
        }
        key
    }

    /// A real, host-native absolute project directory for tests that never
    /// touch the filesystem (pure `FakeProcessRunner` argv assertions): a
    /// hardcoded Unix-style literal like `/tmp/myapp` is only a genuine
    /// absolute path under Unix path rules, so on Windows
    /// [`project_cache_dir`]/[`gradle_wrapper`]'s `std::path::absolute` calls
    /// would silently rewrite it (prefixing the process's current drive)
    /// instead of leaving it unchanged, breaking exact-string assertions
    /// built against the literal. Deriving it from [`std::env::temp_dir`]
    /// keeps it genuinely absolute — and hence a no-op under
    /// `std::path::absolute` — on whichever host runs the test.
    fn fake_project_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("frust-cli-gradle-test-argv-{tag}"))
    }

    #[test]
    fn assemble_runs_gradlew_with_task_props_java_home_env_and_prefixes_lines() {
        let project_dir = fake_project_dir("assemble-basic");
        let runner = FakeProcessRunner::new().with(
            gradlew_argv(
                &project_dir,
                "assembleDebug",
                &[
                    "-Pfrust.targetPlatforms=arm64-v8a",
                    "-Pfrust.splitPerAbi=false",
                ],
            ),
            Output {
                success: true,
                stdout: "> Task :app:assembleDebug\nBUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );
        let mut lines = Vec::new();
        let props = vec![
            "-Pfrust.targetPlatforms=arm64-v8a".to_string(),
            "-Pfrust.splitPerAbi=false".to_string(),
        ];
        let out = assemble(
            &runner,
            &project_dir,
            &project_dir.join("android"),
            "/opt/jdk17",
            "assembleDebug",
            &props,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert!(out.success);
        assert_eq!(
            lines,
            vec![
                "[gradle] > Task :app:assembleDebug",
                "[gradle] BUILD SUCCESSFUL",
            ]
        );
        // Gradle still runs *in* `android/` — only its cache moved.
        assert_eq!(
            runner.recorded_cwd().as_deref(),
            Some(&*project_dir.join("android"))
        );
    }

    /// Pins the exact leading argv and the exact cache directory: Gradle's
    /// per-project cache must land under `<project>/build/android/.gradle`,
    /// absolute, ahead of the task name — `frust clean` removes it by that
    /// same project-rooted path.
    #[test]
    fn assemble_leads_with_an_absolute_project_cache_dir_under_build_android() {
        let project_dir = fake_project_dir("cache-dir");
        // A real, host-native absolute `project_dir` makes `project_cache_dir`'s
        // `std::path::absolute` call a no-op, so the expected cache path can
        // be a plain `Path::join` instead of a hardcoded (Unix-only-absolute)
        // literal — see `fake_project_dir`.
        let expected_cache = project_dir.join(BuildLayout::android_gradle_cache());
        assert_eq!(project_cache_dir(&project_dir), expected_cache);
        assert_eq!(
            leading_args(&project_dir),
            [
                "--project-cache-dir".to_string(),
                expected_cache.display().to_string(),
            ]
        );

        let runner = FakeProcessRunner::new().with(
            gradlew_argv(&project_dir, "assembleRelease", &[]),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );
        let out = assemble(
            &runner,
            &project_dir,
            &project_dir.join("android"),
            "/opt/jdk17",
            "assembleRelease",
            &[],
            &mut |_| {},
        )
        .unwrap();
        assert!(out.success);
    }

    /// A relative `project_dir` still yields an absolute cache path: Gradle
    /// would otherwise resolve it against `android/`, putting the cache back
    /// inside the source tree.
    #[test]
    fn project_cache_dir_is_absolute_even_for_a_relative_project_dir() {
        let cache = project_cache_dir(Path::new("myapp"));
        assert!(cache.is_absolute(), "{}", cache.display());
        assert!(
            cache.ends_with("myapp/build/android/.gradle"),
            "{}",
            cache.display()
        );
    }

    /// A hot session's assemble skips the template's cargo-ndk task with
    /// `-x cargoNdkBuild` right after the task name, the cache flag still
    /// leading and the `-P` properties still trailing; only the exact argv
    /// is registered, so any other shape finds no fixture.
    #[test]
    fn assemble_excluding_skips_the_cargo_ndk_task_after_the_task_name() {
        let project_dir = fake_project_dir("assemble-excluding");
        let runner = FakeProcessRunner::new().with(
            gradlew_argv(
                &project_dir,
                "assembleDebug",
                &[
                    "-x",
                    "cargoNdkBuild",
                    "-Pfrust.targetPlatforms=arm64-v8a",
                    "-Pfrust.splitPerAbi=false",
                ],
            ),
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );
        let props = vec![
            "-Pfrust.targetPlatforms=arm64-v8a".to_string(),
            "-Pfrust.splitPerAbi=false".to_string(),
        ];
        let out = assemble_excluding(
            &runner,
            &project_dir,
            &project_dir.join("android"),
            "/opt/jdk17",
            "assembleDebug",
            &[CARGO_NDK_BUILD_TASK],
            &props,
            &mut |_| {},
        )
        .unwrap();
        assert!(out.success);
        assert_eq!(CARGO_NDK_BUILD_TASK, "cargoNdkBuild");
    }

    #[test]
    fn assemble_surfaces_failure() {
        let project_dir = fake_project_dir("failing-app");
        let runner = FakeProcessRunner::new().with(
            gradlew_argv(&project_dir, "assembleRelease", &[]),
            Output {
                success: false,
                stdout: "> Task :app:compileReleaseKotlin FAILED".to_string(),
                stderr: "e: compile error".to_string(),
            },
        );
        let out = assemble(
            &runner,
            &project_dir,
            &project_dir.join("android"),
            "/opt/jdk17",
            "assembleRelease",
            &[],
            &mut |_| {},
        )
        .unwrap();
        assert!(!out.success);
    }

    #[test]
    fn apk_output_path_finds_the_single_debug_apk_no_flavor() {
        let dir = unique_temp_dir("apk-output-debug");
        let out_dir = dir.join("build/android/app/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();

        let path = apk_output_path(&dir, BuildMode::Debug, None, &mut |_| {}).unwrap();
        assert_eq!(path, out_dir.join("app-debug.apk"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apk_output_path_finds_flavored_release_apk() {
        let dir = unique_temp_dir("apk-output-flavored-release");
        let out_dir = dir.join("build/android/app/outputs/apk/paid/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-paid-release.apk"), b"fake").unwrap();

        let path = apk_output_path(&dir, BuildMode::Release, Some("paid"), &mut |_| {}).unwrap();
        assert_eq!(path, out_dir.join("app-paid-release.apk"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apk_output_path_errs_when_more_than_one_apk_present() {
        let dir = unique_temp_dir("apk-output-ambiguous");
        let out_dir = dir.join("build/android/app/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-arm64-v8a-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("app-x86_64-release.apk"), b"fake").unwrap();

        let err = apk_output_path(&dir, BuildMode::Release, None, &mut |_| {}).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("expected 1"), "{message}");
        assert!(message.contains("found 2"), "{message}");
        assert!(message.contains("frust clean"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-gradle-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn wrapper_dist_cached_false_when_dists_dir_absent() {
        let dir = unique_temp_dir("no-dists");
        fs::create_dir_all(&dir).unwrap();
        assert!(!wrapper_dist_cached(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrapper_dist_cached_true_when_dists_populated() {
        let dir = unique_temp_dir("with-dists");
        let dists = dir.join("wrapper").join("dists").join("gradle-9.5-bin");
        fs::create_dir_all(&dists).unwrap();
        fs::write(dists.join("marker"), b"x").unwrap();
        assert!(wrapper_dist_cached(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Unix arm unchanged: relative `./gradlew`, run with `android_dir` as
    /// `current_dir` — byte-identical to the pre-Windows-support program, so
    /// existing tests/scripts don't move.
    #[test]
    fn gradle_wrapper_is_relative_gradlew_off_windows() {
        let android_dir = Path::new("/tmp/myapp/android");
        assert_eq!(
            gradle_wrapper_for(android_dir, false),
            PathBuf::from("./gradlew")
        );
        assert_eq!(gradle_wrapper_display_for(false), "./gradlew");
    }

    /// Windows arm: the absolute `gradlew.bat` wrapper under `android_dir` —
    /// CreateProcess can't run `./gradlew` (a POSIX shell script) and
    /// Rust's `Command` neither appends `.bat` nor resolves a bare name
    /// against the child's `current_dir`.
    ///
    /// `android_dir` has to be genuinely absolute on whichever host runs
    /// this (see [`fake_project_dir`]): `gradle_wrapper_for` calls
    /// `std::path::absolute`, which is a no-op on a truly absolute input but
    /// would otherwise prefix a Unix-only-absolute literal like
    /// `/tmp/myapp/android` with the process's current drive on a real
    /// Windows host, no longer matching the plain `Path::join` this asserts
    /// against.
    #[test]
    fn gradle_wrapper_is_absolute_gradlew_bat_on_windows() {
        let android_dir = std::path::absolute(Path::new("myapp/android")).unwrap();
        assert_eq!(
            gradle_wrapper_for(&android_dir, true),
            android_dir.join("gradlew.bat")
        );
        assert_eq!(gradle_wrapper_display_for(true), "gradlew.bat");
    }

    /// A relative `android_dir` still yields an absolute Windows wrapper
    /// path — `gradle_wrapper`'s doc promises an absolute program so the
    /// spawn works regardless of the caller's own working directory.
    #[test]
    fn gradle_wrapper_windows_path_is_absolute_even_for_a_relative_android_dir() {
        let wrapper = gradle_wrapper_for(Path::new("myapp/android"), true);
        assert!(wrapper.is_absolute(), "{}", wrapper.display());
        assert!(
            wrapper.ends_with("myapp/android/gradlew.bat"),
            "{}",
            wrapper.display()
        );
    }
}
