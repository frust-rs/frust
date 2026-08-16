//! The compile half of a desktop bundle: the `cargo build` invocation and
//! finding the binary it produced.
//!
//! Both go through the injected [`ProcessRunner`] — the compile as a streamed
//! invocation whose lines reach the caller's `on_line` sink, and the target
//! directory (when the environment doesn't name one) through `cargo metadata`,
//! because the directory cargo actually writes to is a *config* value
//! (`build.target-dir` in any `config.toml` cargo discovers), not merely
//! `<project>/target`. Asking cargo is the only answer that can't drift from
//! where the binary really landed.

use std::path::{Path, PathBuf};

use crate::build_info::{BuildInfo, BuildMode};
use crate::doctor::EnvLookup;
use crate::process::{ProcessRunner, tail_lines};

use super::config::DesktopConfig;
use super::{DesktopBuildError, DesktopBundleTarget};

/// How many trailing output lines a failed compile reports — the same bound
/// the Android/iOS pipelines use for a failed toolchain invocation.
const FAILURE_TAIL_LINES: usize = 50;

/// Runs `cargo build` for `info`'s mode in `project_dir`, streaming each line
/// to `on_line` prefixed with `[cargo]`.
///
/// Mode → flags is [`BuildMode::cargo_profile_arg`] plus the mode's cargo
/// features, resolved through [`crate::cargo_manifest::resolve_release_features`]
/// so a release build of an app predating the `lean` feature drops it (with a
/// one-time warning through the sink) instead of failing on cargo's opaque
/// unknown-feature error. This is the same mapping
/// [`crate::desktop_run::desktop_plan`] builds for `cargo run`, so a bundle
/// and a desktop preview compile the same app.
pub(super) fn build(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
) -> Result<(), DesktopBuildError> {
    let (features, warning) =
        crate::cargo_manifest::resolve_release_features(project_dir, info.mode);
    if let Some(warning) = warning {
        on_line(&warning);
    }

    let mut args: Vec<String> = vec!["build".to_string()];
    for arg in info.mode.cargo_profile_arg() {
        args.push((*arg).to_string());
    }
    for feature in &features {
        args.push("--features".to_string());
        args.push((*feature).to_string());
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let printable = args.join(" ");

    let mut prefixed = |line: &str| on_line(&format!("[cargo] {line}"));
    let out = runner
        .run_streaming("cargo", &argv, Some(project_dir), &[], &mut prefixed)
        .map_err(|err| DesktopBuildError::CargoSpawn {
            args: printable.clone(),
            reason: format!("{err:#}"),
        })?;

    if !out.success {
        let tail = tail_lines(&out.stderr, FAILURE_TAIL_LINES);
        let tail = if tail.is_empty() {
            tail_lines(&out.stdout, FAILURE_TAIL_LINES)
        } else {
            tail
        };
        return Err(DesktopBuildError::CargoFailed {
            args: printable,
            tail,
        });
    }
    Ok(())
}

/// The file name the compiled binary carries for `target` — `.exe`-suffixed
/// for a Windows bundle, bare everywhere else. Keyed on the *bundle target*
/// rather than the host so the layout is decided by what is being assembled
/// (host and target are the same thing in production, but not in this
/// module's tests).
pub(super) fn binary_file_name(target: DesktopBundleTarget, binary: &str) -> String {
    match target {
        DesktopBundleTarget::Windows => format!("{binary}.exe"),
        _ => binary.to_string(),
    }
}

/// Locates the binary a successful [`build`] produced:
/// `<target-dir>/<profile-dir>/<binary>[.exe]`.
pub(super) fn locate_binary(
    runner: &dyn ProcessRunner,
    env: &dyn EnvLookup,
    project_dir: &Path,
    info: &BuildInfo,
    target: DesktopBundleTarget,
    config: &DesktopConfig,
) -> Result<PathBuf, DesktopBuildError> {
    let profile = profile_dir(info.mode);
    let path = resolve_target_dir(runner, env, project_dir)
        .join(profile)
        .join(binary_file_name(target, &config.binary_name));
    if !path.is_file() {
        return Err(DesktopBuildError::BinaryNotFound {
            binary: config.binary_name.clone(),
            profile,
            path,
        });
    }
    Ok(path)
}

/// The directory name cargo puts a mode's artifacts in. `dev` builds land in
/// `debug/`; every other profile's directory is named after the profile.
fn profile_dir(mode: BuildMode) -> &'static str {
    match mode {
        BuildMode::Debug => "debug",
        BuildMode::Profile => "profile",
        BuildMode::Release => "release",
    }
}

/// Where cargo writes artifacts for `project_dir`, in cargo's own precedence
/// order:
///
/// 1. `CARGO_TARGET_DIR` from the environment (relative values are resolved
///    against the project directory — the build's working directory);
/// 2. whatever `cargo metadata` reports, which folds in any `build.target-dir`
///    from cargo's config files (a shared//global target directory is common,
///    and assuming `<project>/target` silently loses the binary on such a
///    machine);
/// 3. `<project>/target`, cargo's default, if metadata can't be read at all.
fn resolve_target_dir(
    runner: &dyn ProcessRunner,
    env: &dyn EnvLookup,
    project_dir: &Path,
) -> PathBuf {
    if let Some(dir) = env.get("CARGO_TARGET_DIR").filter(|dir| !dir.is_empty()) {
        let dir = PathBuf::from(dir);
        return if dir.is_absolute() {
            dir
        } else {
            project_dir.join(dir)
        };
    }

    let manifest_path = project_dir.join("Cargo.toml");
    let manifest_path = manifest_path.to_string_lossy().to_string();
    let metadata = runner.run(
        "cargo",
        &[
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            &manifest_path,
        ],
    );
    if let Ok(out) = metadata
        && out.success
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&out.stdout)
        && let Some(dir) = value.get("target_directory").and_then(|d| d.as_str())
    {
        return PathBuf::from(dir);
    }

    project_dir.join("target")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::BuildArgs;
    use crate::doctor::FakeEnv;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-desktop-cargo-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn info(mode: BuildMode) -> BuildInfo {
        BuildInfo::from_args(BuildArgs::default(), mode).unwrap()
    }

    fn metadata_runner(target_directory: &str) -> FakeProcessRunner {
        FakeProcessRunner::new().with(
            "cargo metadata --no-deps --format-version 1 --manifest-path",
            Output {
                success: true,
                stdout: format!(
                    "{{\"packages\":[],\"target_directory\":\"{target_directory}\",\"version\":1}}"
                ),
                stderr: String::new(),
            },
        )
    }

    #[test]
    fn a_debug_build_uses_no_profile_flag_and_the_debug_directory() {
        let dir = temp_dir("debug-args");
        let runner = FakeProcessRunner::new().with(
            "cargo build --features frust/perf-trace --features frust/devtools",
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        // An unregistered invocation is a hard error in the fake runner, so a
        // green result IS the argv assertion.
        build(&runner, &dir, &info(BuildMode::Debug), &mut |_| {}).unwrap();
        assert_eq!(runner.recorded_cwd(), Some(dir.clone()));
        assert_eq!(profile_dir(BuildMode::Debug), "debug");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_profile_build_passes_the_profile_flag_pair() {
        let dir = temp_dir("profile-args");
        let runner = FakeProcessRunner::new().with(
            "cargo build --profile profile --features frust/perf-trace --features frust/devtools",
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        build(&runner, &dir, &info(BuildMode::Profile), &mut |_| {}).unwrap();
        assert_eq!(profile_dir(BuildMode::Profile), "profile");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A release build of an app that declares no `lean` feature drops it and
    /// warns once through the sink — never handing cargo an undeclared
    /// feature.
    #[test]
    fn a_release_build_against_a_legacy_app_drops_lean_and_warns() {
        let dir = temp_dir("legacy-lean");
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
        let runner = FakeProcessRunner::new().with(
            "cargo build --release",
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let mut lines = Vec::new();
        build(&runner, &dir, &info(BuildMode::Release), &mut |l| {
            lines.push(l.to_string())
        })
        .unwrap();
        assert!(lines.iter().any(|l| l.contains("lean")), "{lines:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cargo_target_dir_from_the_environment_wins() {
        let dir = temp_dir("env-target-dir");
        let runner = metadata_runner("/should/not/be/used");
        let env = FakeEnv::new().set("CARGO_TARGET_DIR", "/elsewhere/target");
        assert_eq!(
            resolve_target_dir(&runner, &env, &dir),
            PathBuf::from("/elsewhere/target")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_relative_cargo_target_dir_resolves_against_the_project() {
        let dir = temp_dir("relative-target-dir");
        let runner = metadata_runner("/should/not/be/used");
        let env = FakeEnv::new().set("CARGO_TARGET_DIR", "build-out");
        assert_eq!(
            resolve_target_dir(&runner, &env, &dir),
            dir.join("build-out")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The case a machine with a shared `build.target-dir` config hits: the
    /// environment names nothing, and the answer is whatever cargo reports —
    /// never `<project>/target`.
    #[test]
    fn a_shared_target_dir_from_cargo_config_is_read_through_cargo_metadata() {
        let dir = temp_dir("config-target-dir");
        let runner = metadata_runner("/data/cache/target");
        assert_eq!(
            resolve_target_dir(&runner, &FakeEnv::new(), &dir),
            PathBuf::from("/data/cache/target")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unavailable_metadata_call_falls_back_to_the_default_target_dir() {
        let dir = temp_dir("default-target-dir");
        let runner = FakeProcessRunner::new();
        assert_eq!(
            resolve_target_dir(&runner, &FakeEnv::new(), &dir),
            dir.join("target")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_windows_bundle_looks_for_an_exe() {
        assert_eq!(
            binary_file_name(DesktopBundleTarget::Windows, "my_app"),
            "my_app.exe"
        );
        assert_eq!(
            binary_file_name(DesktopBundleTarget::Linux, "my_app"),
            "my_app"
        );
        assert_eq!(
            binary_file_name(DesktopBundleTarget::Macos, "my_app"),
            "my_app"
        );
    }
}
