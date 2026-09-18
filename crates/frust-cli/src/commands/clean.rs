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
/// arrives, mapping the result to an exit code (0 in every non-error case).
pub fn run_in(runner: &dyn ProcessRunner, project_dir: &Path) -> Result<u8> {
    let _ = frust_drive::clean::run(runner, project_dir, &mut |line: &str| {
        println!("{line}");
    })?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::process::FakeProcessRunner;
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
}
