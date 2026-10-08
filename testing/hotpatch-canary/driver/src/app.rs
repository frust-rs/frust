//! The running fat image: spawned directly (never `cargo run`), driven over
//! the app's stdin/stdout line protocol (see the app crate's docs).

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

/// How long the app has to answer one request (a patch load included).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);

/// What the app announced at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ready {
    pub pid: u32,
    pub anchor_runtime: u64,
    pub value: u64,
}

/// The app process and its two pipes.
pub struct App {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl App {
    /// Spawns `exe` in `cwd` and reads its `ready` line.
    pub fn spawn(exe: &Path, cwd: &Path) -> Result<(Self, Ready)> {
        let mut child = Command::new(exe)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawning the fat image `{}`", exe.display()))?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let stdin = child.stdin.take();
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut app = Self {
            child,
            stdin,
            lines,
        };
        let line = app.answer()?;
        let ready = parse_ready(&line)?;
        if ready.pid != app.child.id() {
            bail!(
                "the app reports pid {}, the driver spawned pid {}",
                ready.pid,
                app.child.id()
            );
        }
        Ok((app, ready))
    }

    /// Sends one request line and returns the answer.
    pub fn request(&mut self, line: &str) -> Result<String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("the app's stdin is already closed"))?;
        writeln!(stdin, "{line}")
            .and_then(|()| stdin.flush())
            .context("writing to the app's stdin")?;
        self.answer()
    }

    fn answer(&mut self) -> Result<String> {
        match self.lines.recv_timeout(ANSWER_TIMEOUT) {
            Ok(line) => Ok(line),
            Err(RecvTimeoutError::Timeout) => bail!(
                "the app gave no answer within {}s",
                ANSWER_TIMEOUT.as_secs()
            ),
            Err(RecvTimeoutError::Disconnected) => {
                let status = self.child.wait().ok();
                bail!("the app exited without answering ({status:?})")
            }
        }
    }

    /// Closes stdin and waits for the app to exit cleanly.
    pub fn finish(mut self) -> Result<()> {
        drop(self.stdin.take());
        let status = self.child.wait().context("waiting for the app")?;
        if !status.success() {
            bail!("the app exited with {status}");
        }
        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Parses `ready pid=<pid> anchor=<0x..> value=<n>`.
pub fn parse_ready(line: &str) -> Result<Ready> {
    let rest = line
        .strip_prefix("ready ")
        .ok_or_else(|| anyhow!("expected the app's `ready` line, got `{line}`"))?;
    let pid = field(rest, "pid")?
        .parse()
        .with_context(|| format!("bad pid in `{line}`"))?;
    let anchor = field(rest, "anchor")?;
    let anchor_runtime = anchor
        .strip_prefix("0x")
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .filter(|address| *address != 0)
        .ok_or_else(|| anyhow!("bad anchor in `{line}`"))?;
    let value = field(rest, "value")?
        .parse()
        .with_context(|| format!("bad value in `{line}`"))?;
    Ok(Ready {
        pid,
        anchor_runtime,
        value,
    })
}

/// The value of `key=<value>` among the space-separated fields of `line`.
pub fn field<'a>(line: &'a str, key: &str) -> Result<&'a str> {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(key)?.strip_prefix('='))
        .ok_or_else(|| anyhow!("no `{key}=` in `{line}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ready_line_parses() {
        let ready = parse_ready("ready pid=42 anchor=0x10000abc value=1").unwrap();
        assert_eq!(
            ready,
            Ready {
                pid: 42,
                anchor_runtime: 0x1000_0abc,
                value: 1
            }
        );
    }

    #[test]
    fn a_zero_or_missing_anchor_is_refused() {
        assert!(parse_ready("ready pid=42 anchor=0x0 value=1").is_err());
        assert!(parse_ready("ready pid=42 value=1").is_err());
        assert!(parse_ready("refused nope").is_err());
    }

    #[test]
    fn fields_are_matched_by_whole_key() {
        let line = "applied value=2 hits=1 patches=1";
        assert_eq!(field(line, "value").unwrap(), "2");
        assert_eq!(field(line, "patches").unwrap(), "1");
        assert!(field(line, "hit").is_err());
    }
}
