//! `forgekit clean` (spec §12.1 table): removes cargo's build output plus
//! the generated Gradle/Xcode build directories. A no-op-with-message
//! outside a ForgeKit project (no `forgekit.toml`).

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};

use crate::process::{ProcessRunner, RealProcessRunner};

/// Build-output directories removed relative to the project root, beyond
/// `cargo clean`'s own `target/`: the Gradle app-module build dir, the
/// project-local Gradle cache, and `build/` (covers `build/ios`, spec
/// §12.6's `-derivedDataPath`/archive output).
/// Keep in sync with `templates/app/.gitignore`'s build-output patterns.
const REMOVED_DIRS: &[&str] = &["android/app/build", "android/.gradle", "build"];

/// Entry point `commands::dispatch` calls: resolves the real process runner
/// and current directory, then hands off to [`run_in`].
pub fn run() -> Result<u8> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    run_in(&RealProcessRunner, &cwd)
}

/// The testable core of `clean`, taking an injected [`ProcessRunner`] and
/// project directory.
pub fn run_in(runner: &dyn ProcessRunner, project_dir: &Path) -> Result<u8> {
    if !project_dir.join("forgekit.toml").exists() {
        println!(
            "no `forgekit.toml` found in `{}` — nothing to clean.",
            project_dir.display()
        );
        return Ok(0);
    }

    let out = runner
        .run("cargo", &["clean"])
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
    use crate::process::{FakeProcessRunner, Output};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_project_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "forgekit-cli-clean-test-{tag}-{}-{n}",
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
    fn no_op_with_message_outside_a_forgekit_project() {
        let dir = unique_project_dir("no-toml");
        let runner = FakeProcessRunner::new(); // no `cargo clean` fixture registered
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn runs_cargo_clean_through_process_runner() {
        let dir = unique_project_dir("cargo-clean");
        fs::write(
            dir.join("forgekit.toml"),
            "[app]\nname = \"x\"\norg = \"y\"\n",
        )
        .unwrap();
        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn removes_the_three_build_dirs() {
        let dir = unique_project_dir("removes-dirs");
        fs::write(
            dir.join("forgekit.toml"),
            "[app]\nname = \"x\"\norg = \"y\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.join("android/app/build")).unwrap();
        fs::write(dir.join("android/app/build/marker"), "x").unwrap();
        fs::create_dir_all(dir.join("android/.gradle")).unwrap();
        fs::create_dir_all(dir.join("build/ios")).unwrap();

        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 0);

        assert!(!dir.join("android/app/build").exists());
        assert!(!dir.join("android/.gradle").exists());
        assert!(!dir.join("build").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_build_dirs_are_not_an_error() {
        let dir = unique_project_dir("nothing-to-remove");
        fs::write(
            dir.join("forgekit.toml"),
            "[app]\nname = \"x\"\norg = \"y\"\n",
        )
        .unwrap();
        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
