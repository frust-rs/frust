//! Process execution abstraction (spec §12.1). All external tool invocations
//! (`rustc`, `adb`, `xcrun`, `cargo ndk`, …) go through [`ProcessRunner`] so that
//! `doctor`/`devices` (and later `run`/`build`) stay unit-testable without ever
//! shelling out during `cargo test`.

use anyhow::{Context, Result};
use std::process::Command;

#[cfg(test)]
use std::collections::HashMap;

/// Captured result of a process invocation.
///
/// `success` mirrors the process exit status; a non-zero exit is *not* an
/// `Err` from [`ProcessRunner::run`] — only a failure to spawn the process at
/// all (binary missing, no permission, …) is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub trait ProcessRunner {
    fn run(&self, cmd: &str, args: &[&str]) -> Result<Output>;
}

/// Shells out for real via [`std::process::Command`].
pub struct RealProcessRunner;

impl ProcessRunner for RealProcessRunner {
    fn run(&self, cmd: &str, args: &[&str]) -> Result<Output> {
        let out = Command::new(cmd)
            .args(args)
            .output()
            .with_context(|| format!("failed to spawn `{cmd}`"))?;
        Ok(Output {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// A canned response for [`FakeProcessRunner`].
#[cfg(test)]
#[derive(Debug, Clone)]
enum FakeOutcome {
    /// Return this `Output` for the matching invocation.
    Output(Output),
    /// Also write `content` to the path following a `--json-output` flag in
    /// the invocation's args (models `xcrun devicectl … --json-output <file>`).
    OutputWithFile { output: Output, content: String },
    /// Simulate the binary not existing on PATH (spawn failure).
    Missing,
}

/// A fake [`ProcessRunner`] for tests: register canned responses keyed by
/// `"<cmd> <args...>"`. Lookup tries an exact match first, then falls back to
/// the longest registered key that is a prefix of the full invocation — this
/// lets a single registration match invocations with a dynamic trailing
/// argument (e.g. a temp-file path for `devicectl --json-output`).
#[cfg(test)]
#[derive(Default)]
pub struct FakeProcessRunner {
    responses: HashMap<String, FakeOutcome>,
}

#[cfg(test)]
impl FakeProcessRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a response (success or failed-exit) for an exact invocation.
    pub fn with(mut self, key: impl Into<String>, output: Output) -> Self {
        self.responses.insert(key.into(), FakeOutcome::Output(output));
        self
    }

    /// Register a response that also writes `content` to the path following
    /// `--json-output` in the invocation's args.
    pub fn with_file(mut self, key: impl Into<String>, output: Output, content: impl Into<String>) -> Self {
        self.responses
            .insert(key.into(), FakeOutcome::OutputWithFile { output, content: content.into() });
        self
    }

    /// Register `key` as "binary not found" (spawn failure).
    pub fn missing(mut self, key: impl Into<String>) -> Self {
        self.responses.insert(key.into(), FakeOutcome::Missing);
        self
    }
}

#[cfg(test)]
fn invocation_key(cmd: &str, args: &[&str]) -> String {
    std::iter::once(cmd).chain(args.iter().copied()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
impl ProcessRunner for FakeProcessRunner {
    fn run(&self, cmd: &str, args: &[&str]) -> Result<Output> {
        let full = invocation_key(cmd, args);

        let matched = self.responses.get(&full).or_else(|| {
            self.responses
                .iter()
                .filter(|(k, _)| full.starts_with(k.as_str()))
                .max_by_key(|(k, _)| k.len())
                .map(|(_, outcome)| outcome)
        });

        match matched {
            Some(FakeOutcome::Output(output)) => Ok(output.clone()),
            Some(FakeOutcome::OutputWithFile { output, content }) => {
                if let Some(path) =
                    args.iter().position(|a| *a == "--json-output").and_then(|pos| args.get(pos + 1))
                {
                    std::fs::write(path, content)
                        .with_context(|| format!("fake runner: failed to write {path}"))?;
                }
                Ok(output.clone())
            }
            Some(FakeOutcome::Missing) | None => {
                anyhow::bail!("No such file or directory (os error 2): {cmd}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_returns_registered_output() {
        let runner = FakeProcessRunner::new().with(
            "rustc --version",
            Output { success: true, stdout: "rustc 1.91.1\n".into(), stderr: String::new() },
        );
        let out = runner.run("rustc", &["--version"]).unwrap();
        assert!(out.success);
        assert_eq!(out.stdout, "rustc 1.91.1\n");
    }

    #[test]
    fn missing_binary_errs() {
        let runner = FakeProcessRunner::new().missing("cargo ndk --version");
        assert!(runner.run("cargo", &["ndk", "--version"]).is_err());
    }

    #[test]
    fn unregistered_invocation_errs() {
        let runner = FakeProcessRunner::new();
        assert!(runner.run("adb", &["devices", "-l"]).is_err());
    }

    #[test]
    fn prefix_match_handles_dynamic_trailing_arg() {
        let runner = FakeProcessRunner::new().with_file(
            "xcrun devicectl list devices --json-output",
            Output { success: true, stdout: String::new(), stderr: String::new() },
            r#"{"result":{"devices":[]}}"#,
        );
        let tmp = std::env::temp_dir().join("forgekit-test-devicectl-prefix-match.json");
        let tmp_str = tmp.to_string_lossy().to_string();
        let out = runner
            .run("xcrun", &["devicectl", "list", "devices", "--json-output", &tmp_str])
            .unwrap();
        assert!(out.success);
        let content = std::fs::read_to_string(&tmp).unwrap();
        assert!(content.contains("devices"));
        let _ = std::fs::remove_file(&tmp);
    }
}
