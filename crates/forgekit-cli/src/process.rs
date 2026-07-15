//! Process execution abstraction (spec §12.1). All external tool invocations
//! (`rustc`, `adb`, `xcrun`, `cargo ndk`, …) go through [`ProcessRunner`] so that
//! `doctor`/`devices` (and later `run`/`build`) stay unit-testable without ever
//! shelling out during `cargo test`.

use anyhow::{Context, Result};
use std::path::Path;
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

    /// Like [`run`](ProcessRunner::run), but for long-running/streaming
    /// invocations (`./gradlew`, `adb logcat`, `cargo run`) where the caller
    /// wants each line of stdout as it arrives rather than only the final
    /// buffered [`Output`]. `cwd` sets the child's working directory (`None`
    /// = inherit); `env` adds/overrides environment variables for the child
    /// only (e.g. `JAVA_HOME` for a `./gradlew` invocation) without touching
    /// the parent process's environment.
    fn run_streaming(
        &self,
        cmd: &str,
        args: &[&str],
        cwd: Option<&Path>,
        env: &[(&str, &str)],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<Output>;
}

/// Strips a trailing `\n` (and a preceding `\r`, for CRLF-terminated
/// output) from a `read_until(b'\n', ..)`-read line, then lossy-decodes the
/// remaining bytes to a `String` — mirrors [`RealProcessRunner::run`]'s
/// `String::from_utf8_lossy` behavior so `run_streaming` never aborts on
/// non-UTF-8 byte sequences (e.g. from `logcat`/`gradlew`) the way
/// `BufRead::lines()`'s `io::Error` on invalid UTF-8 would.
fn decode_stream_line(mut raw: &[u8]) -> String {
    if raw.last() == Some(&b'\n') {
        raw = &raw[..raw.len() - 1];
        if raw.last() == Some(&b'\r') {
            raw = &raw[..raw.len() - 1];
        }
    }
    String::from_utf8_lossy(raw).into_owned()
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

    fn run_streaming(
        &self,
        cmd: &str,
        args: &[&str],
        cwd: Option<&Path>,
        env: &[(&str, &str)],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<Output> {
        use std::io::{BufRead, BufReader, Read};
        use std::process::Stdio;

        let mut command = Command::new(cmd);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        for (key, value) in env {
            command.env(key, value);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn `{cmd}`"))?;

        // Drain stderr on a background thread so a child that fills its
        // stderr pipe while we're blocked reading stdout can't deadlock us.
        // Read raw bytes to EOF and lossy-decode once, rather than
        // `read_to_string`, which silently truncates on invalid UTF-8.
        let stderr_handle = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = stderr.read_to_end(&mut buf);
                String::from_utf8_lossy(&buf).into_owned()
            })
        });

        let mut stdout_lines = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            let mut reader = BufReader::new(stdout);
            let mut raw = Vec::new();
            loop {
                raw.clear();
                let n = reader
                    .read_until(b'\n', &mut raw)
                    .with_context(|| format!("reading stdout from `{cmd}`"))?;
                if n == 0 {
                    break;
                }
                let line = decode_stream_line(&raw);
                on_line(&line);
                stdout_lines.push(line);
            }
        }

        let stderr = stderr_handle
            .map(|handle| handle.join().unwrap_or_default())
            .unwrap_or_default();
        let status = child
            .wait()
            .with_context(|| format!("waiting on `{cmd}`"))?;

        Ok(Output {
            success: status.success(),
            stdout: stdout_lines.join("\n"),
            stderr,
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
        self.responses
            .insert(key.into(), FakeOutcome::Output(output));
        self
    }

    /// Register a response that also writes `content` to the path following
    /// `--json-output` in the invocation's args.
    pub fn with_file(
        mut self,
        key: impl Into<String>,
        output: Output,
        content: impl Into<String>,
    ) -> Self {
        self.responses.insert(
            key.into(),
            FakeOutcome::OutputWithFile {
                output,
                content: content.into(),
            },
        );
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
    std::iter::once(cmd)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
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
                if let Some(path) = args
                    .iter()
                    .position(|a| *a == "--json-output")
                    .and_then(|pos| args.get(pos + 1))
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

    /// Fakes streaming by resolving the invocation exactly as [`run`](Self::run)
    /// does, then replaying its `stdout` one line at a time through
    /// `on_line` before returning the same [`Output`]. `cwd`/`env` are
    /// ignored — invocation matching is keyed on `cmd`/`args` only, same as
    /// `run`.
    fn run_streaming(
        &self,
        cmd: &str,
        args: &[&str],
        _cwd: Option<&Path>,
        _env: &[(&str, &str)],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<Output> {
        let out = self.run(cmd, args)?;
        for line in out.stdout.lines() {
            on_line(line);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_stream_line_replaces_invalid_utf8_lossily() {
        // 0xff is never valid as a UTF-8 lead byte.
        let raw: &[u8] = &[b'h', b'i', 0xff, b'!', b'\n'];
        let decoded = decode_stream_line(raw);
        assert_eq!(decoded, "hi\u{FFFD}!");
    }

    #[test]
    fn decode_stream_line_strips_trailing_newline_and_cr() {
        assert_eq!(decode_stream_line(b"plain\n"), "plain");
        assert_eq!(decode_stream_line(b"crlf\r\n"), "crlf");
        assert_eq!(decode_stream_line(b"no-newline"), "no-newline");
    }

    #[test]
    fn run_streaming_handles_invalid_utf8_from_a_real_process() {
        // Spawns a real child (`sh -c printf`) that writes an invalid UTF-8
        // byte to stdout, proving `run_streaming` lossy-decodes it instead
        // of erroring the way `BufRead::lines()` would.
        let runner = RealProcessRunner;
        let mut seen = Vec::new();
        let out = runner
            .run_streaming(
                "/bin/sh",
                &["-c", r"printf 'before\xffafter\n'"],
                None,
                &[],
                &mut |line| seen.push(line.to_string()),
            )
            .unwrap();
        assert!(out.success);
        assert_eq!(seen, vec!["before\u{FFFD}after"]);
        assert_eq!(out.stdout, "before\u{FFFD}after");
    }

    #[test]
    fn exact_match_returns_registered_output() {
        let runner = FakeProcessRunner::new().with(
            "rustc --version",
            Output {
                success: true,
                stdout: "rustc 1.91.1\n".into(),
                stderr: String::new(),
            },
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
    fn run_streaming_replays_stdout_lines_then_returns_output() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 logcat --pid 1234",
            Output {
                success: true,
                stdout: "line one\nline two\n".to_string(),
                stderr: String::new(),
            },
        );
        let mut seen = Vec::new();
        let out = runner
            .run_streaming(
                "adb",
                &["-s", "emulator-5554", "logcat", "--pid", "1234"],
                None,
                &[],
                &mut |line| seen.push(line.to_string()),
            )
            .unwrap();
        assert_eq!(seen, vec!["line one", "line two"]);
        assert!(out.success);
    }

    #[test]
    fn prefix_match_handles_dynamic_trailing_arg() {
        let runner = FakeProcessRunner::new().with_file(
            "xcrun devicectl list devices --json-output",
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
            r#"{"result":{"devices":[]}}"#,
        );
        let tmp = std::env::temp_dir().join("forgekit-test-devicectl-prefix-match.json");
        let tmp_str = tmp.to_string_lossy().to_string();
        let out = runner
            .run(
                "xcrun",
                &["devicectl", "list", "devices", "--json-output", &tmp_str],
            )
            .unwrap();
        assert!(out.success);
        let content = std::fs::read_to_string(&tmp).unwrap();
        assert!(content.contains("devices"));
        let _ = std::fs::remove_file(&tmp);
    }
}
