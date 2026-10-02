//! `frust clean`: removes cargo's build output plus
//! the generated Gradle/Xcode build directories. A no-op-with-message
//! outside a Frust project (no `frust.toml`).
//!
//! Delegates to [`frust_drive::clean::run`] for the actual work; see
//! [`frust_drive::build_dirs`] for the directory list.

use std::path::Path;

use anyhow::Result;

use frust_drive::process::ProcessRunner;

/// The testable core of `clean`, taking an injected [`ProcessRunner`] and
/// project directory. `commands::dispatch` constructs the real runner and
/// current directory and calls this (the CLI's one `Real` construction
/// site).
///
/// Delegates to [`frust_drive::clean::run`] and prints each line as it
/// arrives, mapping the result to an exit code:
///
/// - [`CleanReport::NotAFrustProject`] and
///   [`CleanReport::Cleaned`]`{ cargo_clean_succeeded: true }` both exit 0 —
///   there was nothing to clean, or cleaning succeeded.
/// - [`CleanReport::Cleaned`]`{ cargo_clean_succeeded: false }` exits 1 —
///   `cargo clean` itself failed; its failure line was already printed by
///   the `on_line` sink below, so this only decides the process's exit
///   status, not what the user saw.
///
/// [`CleanReport::NotAFrustProject`]: frust_drive::clean::CleanReport::NotAFrustProject
/// [`CleanReport::Cleaned`]: frust_drive::clean::CleanReport::Cleaned
pub fn run_in(runner: &dyn ProcessRunner, project_dir: &Path) -> Result<u8> {
    let report = frust_drive::clean::run(runner, project_dir, &mut |line: &str| {
        println!("{line}");
    })?;
    let exit_code = match report {
        frust_drive::clean::CleanReport::Cleaned {
            cargo_clean_succeeded: false,
        } => 1,
        frust_drive::clean::CleanReport::Cleaned {
            cargo_clean_succeeded: true,
        }
        | frust_drive::clean::CleanReport::NotAFrustProject => 0,
    };
    Ok(exit_code)
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
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_manifest(dir: &Path) {
        std::fs::write(dir.join("frust.toml"), "[app]\nname = \"x\"\norg = \"y\"\n").unwrap();
    }

    /// CLI-level test: exit code 0 when invoked outside a Frust project.
    /// The core behavior and message output are tested in `frust-drive`'s
    /// own test suite.
    #[test]
    fn no_op_exit_code_outside_a_frust_project() {
        let dir = unique_project_dir("no-toml");
        let runner = FakeProcessRunner::new();
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The exit-code half of the `CleanReport` contract documented on
    /// [`run_in`]: a failed `cargo clean` exits 1, not 0.
    #[test]
    fn exits_1_when_cargo_clean_fails() {
        let dir = unique_project_dir("cargo-clean-failed");
        write_manifest(&dir);
        let failed = Output {
            success: false,
            stdout: String::new(),
            stderr: "boom".to_string(),
        };
        let runner = FakeProcessRunner::new().with("cargo clean", failed);
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `run_in` must run `cargo clean` in `project_dir`, not the process's
    /// own `cwd` — asserted via `FakeProcessRunner::recorded_cwd`. Ports
    /// `frust-drive::clean`'s identically-shaped regression test at the CLI
    /// call site.
    #[test]
    fn runs_cargo_clean_in_the_project_dir_not_the_process_cwd() {
        let dir = unique_project_dir("cargo-clean-cwd");
        write_manifest(&dir);
        assert_ne!(dir, std::env::current_dir().unwrap());

        let ok = Output {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        };
        let runner = FakeProcessRunner::new().with("cargo clean", ok);
        let code = run_in(&runner, &dir).unwrap();
        assert_eq!(code, 0);
        assert_eq!(runner.recorded_cwd(), Some(dir.clone()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
