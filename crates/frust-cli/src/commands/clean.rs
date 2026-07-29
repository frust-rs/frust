//! `frust clean` (spec §12.1 table): removes cargo's build output plus
//! the generated Gradle/Xcode build directories. A no-op-with-message
//! outside a Frust project (no `frust.toml`).

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};

use frust_drive::process::ProcessRunner;

/// Build-output directories removed relative to the project root, beyond
/// `cargo clean`'s own `target/`: the Gradle app-module build dir, the
/// Gradle root-project build dir (where the generated
/// `android/settings.gradle.kts` redirects the `:frust-embedding` embedding
/// module's output, keeping the shared frust checkout pristine), the
/// project-local Gradle cache, and `build/` (covers `build/ios`, spec
/// §12.6's `-derivedDataPath`/archive output).
/// Keep in sync with `templates/app/.gitignore`'s build-output patterns.
const REMOVED_DIRS: &[&str] = &[
    "android/app/build",
    "android/build",
    "android/.gradle",
    "build",
];

/// The testable core of `clean`, taking an injected [`ProcessRunner`] and
/// project directory. `commands::dispatch` constructs the real runner and
/// current directory and calls this (the CLI's one `Real` construction
/// site).
///
/// Runs `cargo clean` through [`ProcessRunner::run_streaming`] with `cwd` set
/// to `project_dir` — `run` has no `cwd` argument and would run against the
/// CLI process's own working directory instead of `project_dir` if the two
/// ever differ (e.g. a future `frust clean --project <dir>` from elsewhere).
/// The streaming path's `on_line` sink prints each line as it arrives —
/// byte-compatible with the prior behavior since `cargo clean` emits nothing
/// to stdout by default.
pub fn run_in(runner: &dyn ProcessRunner, project_dir: &Path) -> Result<u8> {
    if !project_dir.join("frust.toml").exists() {
        println!(
            "no `frust.toml` found in `{}` — nothing to clean.",
            project_dir.display()
        );
        return Ok(0);
    }

    let out = runner
        .run_streaming(
            "cargo",
            &["clean"],
            Some(project_dir),
            &[],
            &mut |line: &str| println!("{line}"),
        )
        .context("failed to run `cargo clean`")?;
    if out.success {
        println!("Removed cargo build artifacts (`cargo clean`).");
    } else {
        println!("`cargo clean` failed:\n{}", out.stderr.trim());
    }

    for rel in REMOVED_DIRS {
        let path = project_dir.join(rel);
        match fs::remove_dir_all(&path) {
            Ok(()) => println!("Removed `{}`.", path.display()),
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("removing `{}`", path.display())),
        }
    }

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::process::{FakeProcessRunner, Output};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_project_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-clean-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ok() -> Output {
        Output {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    #[test]
    fn no_op_with_message_outside_a_frust_project() {
        let dir = unique_project_dir("no-toml");
        let runner = FakeProcessRunner::new(); // no `cargo clean` fixture registered
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn runs_cargo_clean_through_process_runner() {
        let dir = unique_project_dir("cargo-clean");
        fs::write(dir.join("frust.toml"), "[app]\nname = \"x\"\norg = \"y\"\n").unwrap();
        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Regression for the clean-runs-in-the-wrong-directory bug: `run_in`
    /// must route `cargo clean` through `project_dir`, not the CLI process's
    /// own `cwd` — asserted via `FakeProcessRunner::recorded_cwd`, parallel to
    /// `frust-tui`'s `run_clean_runs_cargo_clean_in_the_project_dir_not_the_process_cwd`.
    #[test]
    fn runs_cargo_clean_in_the_project_dir_not_the_process_cwd() {
        let dir = unique_project_dir("cargo-clean-cwd");
        fs::write(dir.join("frust.toml"), "[app]\nname = \"x\"\norg = \"y\"\n").unwrap();
        // Sanity: the fixture directory is not the process's own cwd — proves
        // a bare (cwd-agnostic) `run` couldn't have hit this directory.
        assert_ne!(dir, std::env::current_dir().unwrap());

        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 0);
        assert_eq!(runner.recorded_cwd(), Some(dir.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn removes_the_build_dirs() {
        let dir = unique_project_dir("removes-dirs");
        fs::write(dir.join("frust.toml"), "[app]\nname = \"x\"\norg = \"y\"\n").unwrap();
        fs::create_dir_all(dir.join("android/app/build")).unwrap();
        fs::write(dir.join("android/app/build/marker"), "x").unwrap();
        // The `:frust-embedding` module's redirected Gradle output.
        fs::create_dir_all(dir.join("android/build/frust-embedding")).unwrap();
        fs::create_dir_all(dir.join("android/.gradle")).unwrap();
        fs::create_dir_all(dir.join("build/ios")).unwrap();

        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 0);

        assert!(!dir.join("android/app/build").exists());
        assert!(!dir.join("android/build").exists());
        assert!(!dir.join("android/.gradle").exists());
        assert!(!dir.join("build").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_build_dirs_are_not_an_error() {
        let dir = unique_project_dir("nothing-to-remove");
        fs::write(dir.join("frust.toml"), "[app]\nname = \"x\"\norg = \"y\"\n").unwrap();
        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
