//! Process execution abstraction (spec §12.1). All external tool invocations
//! (`rustc`, `adb`, `xcrun`, `cargo ndk`, …) go through [`ProcessRunner`] so that
//! `doctor`/`devices` (and later `run`/`build`) stay unit-testable without ever
//! shelling out during `cargo test`.

use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

#[cfg(any(test, feature = "test-util"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "test-util"))]
use std::path::PathBuf;
#[cfg(any(test, feature = "test-util"))]
use std::sync::mpsc;
#[cfg(any(test, feature = "test-util"))]
use std::time::Duration;

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

    /// Like [`run_streaming`](ProcessRunner::run_streaming), but
    /// **non-blocking**: spawns the process and returns immediately with a
    /// [`StreamHandle`] a caller drains/cancels at its own pace instead of
    /// blocking the calling thread for the process's whole lifetime. This is
    /// the seam a cancellable, long-running session (the TUI's supervised
    /// `run`/`logcat`/install streams) is built on; `run_streaming` stays the
    /// right choice for a one-shot CLI invocation that just wants to print
    /// lines as they arrive and block until done.
    fn spawn_streaming(
        &self,
        cmd: &str,
        args: &[&str],
        cwd: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Result<StreamHandle>;
}

/// Cap on the number of buffered stdout lines a [`StreamHandle`] retains
/// before dropping the oldest — bounds memory for a long-running streamed
/// child (`adb logcat`, a `--watch`ed `cargo run`) whose consumer falls
/// behind or never drains at all. Chosen generously (a `logcat`/build log is
/// rarely anywhere near this deep between drains) rather than tuned tightly;
/// revisit if a real session's `dropped_lines()` ever reports non-zero.
const LINE_BUFFER_CAP: usize = 10_000;

/// Shared state behind a [`StreamHandle`]'s [`LineReceiver`]: a bounded ring
/// buffer plus a closed flag the reader thread sets once the child's stdout
/// hits EOF (or, for [`FakeProcessRunner`], once its scripted lines are
/// exhausted).
struct LineBuffer {
    queue: VecDeque<String>,
    closed: bool,
    dropped: u64,
}

/// The `Mutex`+`Condvar` pair a [`LineReceiver`] blocks on and a producer
/// (the reader thread) pushes into — never blocks the producer: a push at
/// [`LINE_BUFFER_CAP`] drops the oldest buffered line instead of waiting for
/// a consumer to make room, and the lock itself is only ever held for a
/// short, constant-time critical section (never across a blocking read or
/// wait), so [`StreamHandle::kill`] can never deadlock against it either.
struct LineBufferShared {
    state: Mutex<LineBuffer>,
    ready: Condvar,
}

impl LineBufferShared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(LineBuffer {
                queue: VecDeque::new(),
                closed: false,
                dropped: 0,
            }),
            ready: Condvar::new(),
        })
    }

    /// Producer side: pushes `line`, dropping the oldest buffered line
    /// (incrementing [`dropped_lines`](LineReceiver::dropped_lines)) if the
    /// buffer is already at [`LINE_BUFFER_CAP`]. Never blocks.
    fn push(&self, line: String) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.queue.len() >= LINE_BUFFER_CAP {
            state.queue.pop_front();
            state.dropped += 1;
        }
        state.queue.push_back(line);
        drop(state);
        self.ready.notify_one();
    }

    /// Producer side: marks the buffer closed — no more lines are coming.
    /// Called exactly once, after the reader thread's stdout loop ends.
    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.closed = true;
        drop(state);
        self.ready.notify_all();
    }
}

/// Mirrors [`std::sync::mpsc::RecvError`]'s single "closed with nothing left
/// buffered" case — [`LineReceiver::recv`]'s error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("stream closed with no lines left buffered")]
pub struct RecvError;

/// Mirrors [`std::sync::mpsc::TryRecvError`] — [`LineReceiver::try_recv`]'s
/// error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TryRecvError {
    /// Nothing buffered right now, but the producer may still send more.
    #[error("no line buffered right now")]
    Empty,
    /// Closed (producer done) with nothing left buffered.
    #[error("stream closed with no lines left buffered")]
    Disconnected,
}

/// A bounded, drop-oldest receive handle over a [`StreamHandle`]'s buffered
/// stdout lines (see [`LINE_BUFFER_CAP`]) — replaces the previous unbounded
/// `std::sync::mpsc::Receiver<String>`. Shape-compatible with the subset of
/// `mpsc::Receiver`'s API this crate's consumers use: `recv`, `try_recv`,
/// `iter`/`IntoIterator`. Cloneable (it is an `Arc` over the shared ring):
/// clones COMPETE for lines (each line is delivered to exactly one
/// receiver), so hand exactly one clone to whichever thread drains.
#[derive(Clone)]
pub struct LineReceiver {
    shared: Arc<LineBufferShared>,
}

impl LineReceiver {
    /// Blocks until a line is available, or returns `Err` once the buffer is
    /// closed with nothing left queued.
    pub fn recv(&self) -> Result<String, RecvError> {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(line) = state.queue.pop_front() {
                return Ok(line);
            }
            if state.closed {
                return Err(RecvError);
            }
            state = self
                .shared
                .ready
                .wait(state)
                .unwrap_or_else(|p| p.into_inner());
        }
    }

    /// Non-blocking: the next buffered line, [`TryRecvError::Empty`] if none
    /// are buffered right now but the producer is still alive, or
    /// [`TryRecvError::Disconnected`] if closed with nothing left.
    pub fn try_recv(&self) -> Result<String, TryRecvError> {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(line) = state.queue.pop_front() {
            return Ok(line);
        }
        if state.closed {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }

    /// Blocking iterator over every remaining line, ending once the buffer
    /// closes with nothing left queued — mirrors
    /// `mpsc::Receiver::iter`/`into_iter`.
    pub fn iter(&self) -> LineIter<'_> {
        LineIter { receiver: self }
    }

    /// The number of buffered lines dropped so far because the ring buffer
    /// was at [`LINE_BUFFER_CAP`] when a new line arrived. Additive — never
    /// resets — so a caller can watch it stay at `0` for a healthy session
    /// or notice it climb for one whose consumer can't keep up.
    pub fn dropped_lines(&self) -> u64 {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .dropped
    }
}

/// [`LineReceiver::iter`]'s iterator type.
pub struct LineIter<'a> {
    receiver: &'a LineReceiver,
}

impl Iterator for LineIter<'_> {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        self.receiver.recv().ok()
    }
}

impl<'a> IntoIterator for &'a LineReceiver {
    type Item = String;
    type IntoIter = LineIter<'a>;

    fn into_iter(self) -> LineIter<'a> {
        self.iter()
    }
}

/// A running (or hung/exited) process spawned by
/// [`ProcessRunner::spawn_streaming`].
///
/// **Drop behavior: detach, not kill-on-drop.** Dropping a `StreamHandle`
/// without calling [`kill`](Self::kill) never blocks and never leaves a
/// zombie: the background reader thread that owns the child keeps running
/// undisturbed (a bare `JoinHandle` drop detaches rather than joins), reads
/// stdout to EOF, and always calls `Child::wait` itself once EOF is reached
/// — reaping the process whether it exited on its own or the caller lost
/// interest. A caller that actually wants the process to *stop* (not just be
/// abandoned) must call [`kill`](Self::kill) explicitly — the TUI
/// supervisor's job on session close/quit, not this seam's.
///
/// The line buffer (`lines`) is a bounded, drop-oldest ring buffer (see
/// [`LINE_BUFFER_CAP`]), so a producer that outruns a slow/absent consumer
/// never blocks the reader thread trying to push a line — a push at
/// capacity drops the oldest buffered line and increments
/// [`dropped_lines`](Self::dropped_lines) instead. This is also why `kill`
/// can never deadlock against a full buffer: nothing ever blocks trying to
/// fill it.
pub struct StreamHandle {
    /// Each line of the process's stdout AND stderr (merged/interleaved), in
    /// arrival order, as it streams in, bounded to the last
    /// [`LINE_BUFFER_CAP`] lines. Closes (further `recv`/`try_recv` calls
    /// return `Err`) once both streams have hit EOF and the child is reaped.
    pub lines: LineReceiver,
    kill_action: Option<Box<dyn FnOnce() + Send>>,
    worker: Option<thread::JoinHandle<bool>>,
    result: Option<bool>,
}

impl StreamHandle {
    /// Best-effort-terminates the underlying process, then blocks until the
    /// reader thread (which reaps the child after its stdout hits EOF)
    /// finishes — promptly, since killing the process closes its stdout
    /// pipe and unblocks the reader's read loop. **Idempotent**: a second
    /// call (or a call after the process has already exited and been
    /// reaped via [`wait`](Self::wait)) is a no-op that returns the same
    /// cached result.
    pub fn kill(&mut self) {
        if let Some(action) = self.kill_action.take() {
            action();
        }
        self.join_worker();
    }

    /// Blocks until the process has exited — naturally, or because of a
    /// prior [`kill`](Self::kill) — and returns whether it exited
    /// successfully (`false` for a killed process, mirroring a non-zero
    /// exit). Safe to call more than once; the result is cached after the
    /// first call.
    pub fn wait(&mut self) -> bool {
        self.join_worker();
        self.result.unwrap_or(false)
    }

    /// The number of stdout lines dropped so far because the buffer was at
    /// [`LINE_BUFFER_CAP`] when a new line arrived — see [`LineReceiver::dropped_lines`].
    pub fn dropped_lines(&self) -> u64 {
        self.lines.dropped_lines()
    }

    fn join_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.result = worker.join().ok();
        }
    }
}

/// Returns the last `n` non-empty lines of `s.trim()`, joined by `\n`. Shared
/// by every drive pipeline (`android_run`, `ios_run`) that needs to surface a
/// bounded tail of a failed build's buffered output instead of the caller
/// scrolling past the full log.
pub(crate) fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.trim().lines().filter(|line| !line.is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
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

/// The stdio configuration every streaming spawn
/// ([`RealProcessRunner::run_streaming`] and
/// [`RealProcessRunner::spawn_streaming`]) applies to its child: BOTH stdout
/// AND stderr piped, never inherited.
///
/// This is load-bearing for the TUI. An inherited stdout/stderr writes the
/// child's raw bytes straight to the parent's terminal — and while `frust tui`
/// holds that terminal in raw mode, those bytes bypass ratatui entirely and
/// garble the whole screen (the classic LF-without-CR diagonal staircase a
/// leaked child produces). Both streaming spawns therefore pipe both streams so
/// no child byte ever reaches the tty; a caller surfaces them through the line
/// callback / [`StreamHandle`] instead. Centralized in one const + helper
/// ([`apply_streaming_stdio`]) so the invariant is asserted in one unit test
/// (`streaming_spawns_pipe_both_streams`) and can't silently regress to
/// `Stdio::inherit()` at either spawn site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StreamStdio {
    stdout_piped: bool,
    stderr_piped: bool,
}

/// The one stdio config every TUI-reachable streaming spawn uses — see
/// [`StreamStdio`].
const STREAMING_STDIO: StreamStdio = StreamStdio {
    stdout_piped: true,
    stderr_piped: true,
};

/// Apply [`STREAMING_STDIO`] to `command`'s stdout/stderr.
fn apply_streaming_stdio(command: &mut Command) {
    use std::process::Stdio;
    let stdout = if STREAMING_STDIO.stdout_piped {
        Stdio::piped()
    } else {
        Stdio::inherit()
    };
    let stderr = if STREAMING_STDIO.stderr_piped {
        Stdio::piped()
    } else {
        Stdio::inherit()
    };
    command.stdout(stdout).stderr(stderr);
}

// POSIX `kill(2)`, declared directly rather than taking a `libc` dependency
// for this one syscall (target-gated behind `#[cfg(unix)]` all the same).
// Used by `group_kill_unix` to signal a whole process group; the tests
// additionally use it with signal `0` to probe a grandchild's liveness.
#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// `SIGKILL`s the entire Unix process group whose id equals `child_pid`.
///
/// [`ProcessRunner::spawn_streaming`] places each streamed child in its own
/// process group (`pgid == child pid`, via `CommandExt::process_group(0)`), so
/// the compiled preview binary a `cargo run` child forks is a *grandchild* in
/// that same group. `std::process::Child::kill` signals only the direct child,
/// orphaning the grandchild — the visible bug this fixes (a killed/relaunched
/// `cargo run` left its preview window alive). Signalling the *negative* pgid
/// reaches every process in the group at once.
///
/// **Unix-only.** Windows has no process-group signal; a job-object equivalent
/// is a tracked fast-follow, so today's Windows path keeps the pre-existing
/// direct-child-only `Child::kill` (same gap it already had).
#[cfg(unix)]
fn group_kill_unix(child_pid: u32) {
    // SIGKILL is 9 across every Unix target Frust builds for (Linux, macOS,
    // the BSDs) — a stable kernel-ABI number, not a libc-version detail.
    const SIGKILL: i32 = 9;
    // pgid == the child's own pid (set by `process_group(0)`); the negative
    // sign turns "this pid" into "this whole process group".
    let pgid = child_pid as i32;

    // # Safety
    // `kill` is a thin POSIX syscall wrapper: it takes two `int`s by value and
    // returns an `int`, touching no caller-owned memory, so the call is always
    // memory-safe. A pgid whose group has already exited yields `-1`/`ESRCH` —
    // a defined, benign result, never UB — so a race against the group dying on
    // its own is a harmless no-op (best-effort, hence the ignored return).
    unsafe {
        kill(-pgid, SIGKILL);
    }
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

        let mut command = Command::new(cmd);
        command.args(args);
        // Both streams piped — never inherited — so no child byte reaches the
        // (possibly raw-mode TUI) tty; see `apply_streaming_stdio`.
        apply_streaming_stdio(&mut command);
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

    fn spawn_streaming(
        &self,
        cmd: &str,
        args: &[&str],
        cwd: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Result<StreamHandle> {
        use std::io::{BufRead, BufReader};

        let mut command = Command::new(cmd);
        command.args(args);
        // Both streams piped — never inherited — so no child byte reaches the
        // (possibly raw-mode TUI) tty; see `apply_streaming_stdio`. stderr is
        // then merged into the same line ring below rather than discarded.
        apply_streaming_stdio(&mut command);
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        for (key, value) in env {
            command.env(key, value);
        }

        // Put the child in its own process group so a later [`kill`] can reach
        // the whole tree (`cargo run` + the compiled preview binary it forks),
        // not just the direct child — see [`group_kill_unix`]. Windows has no
        // equivalent here yet (tracked job-object fast-follow); it keeps the
        // pre-existing direct-child-only kill below.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn `{cmd}`"))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        // Captured now (before the child moves into the shared `Arc<Mutex>`)
        // so the group kill below can derive the pgid. On Unix the pgid equals
        // this pid, set by `process_group(0)` above.
        #[cfg(unix)]
        let child_pid = child.id();
        // The child itself is shared: the reader thread below waits on
        // (reaps) it once stdout hits EOF, while `kill` locks it briefly
        // just to send the kill signal. Killing can only ever contend for
        // this lock during the reader thread's own final `wait` call —
        // never block on it, since that `wait` doesn't block once the
        // child has actually exited (which killing it just caused).
        let child = Arc::new(Mutex::new(child));

        let shared = LineBufferShared::new();
        let shared_producer = Arc::clone(&shared);
        let stderr_producer = Arc::clone(&shared);

        let wait_child = Arc::clone(&child);
        let worker = thread::spawn(move || {
            // Drain stderr on its own thread (same deadlock-avoidance reason as
            // `run_streaming` above), MERGING each stderr line into the same
            // line ring as stdout — an interactive session (`simctl launch
            // --console-pty`, `devicectl … --console`, a `cargo run` preview)
            // writes to both, and a TUI tab that showed only stdout would drop
            // half the output. Unlabeled/interleaved on purpose (the drive
            // phases prefix their own lines; per-line stderr tagging here would
            // just be noise). Never leaked to the tty — both are piped.
            let stderr_drain = stderr.map(|stderr| {
                thread::spawn(move || {
                    let mut reader = BufReader::new(stderr);
                    let mut raw = Vec::new();
                    loop {
                        raw.clear();
                        match reader.read_until(b'\n', &mut raw) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => stderr_producer.push(decode_stream_line(&raw)),
                        }
                    }
                })
            });

            if let Some(stdout) = stdout {
                let mut reader = BufReader::new(stdout);
                let mut raw = Vec::new();
                loop {
                    raw.clear();
                    match reader.read_until(b'\n', &mut raw) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let line = decode_stream_line(&raw);
                            // Bounded, drop-oldest push — never blocks even
                            // if the consumer has fallen behind or the
                            // `StreamHandle` itself was dropped (see
                            // `LineBufferShared::push`).
                            shared_producer.push(line);
                        }
                    }
                }
            }

            if let Some(handle) = stderr_drain {
                let _ = handle.join();
            }

            let success = wait_child
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .wait()
                .map(|status| status.success())
                .unwrap_or(false);
            // Close only now, mirroring the old unbounded-mpsc sender's
            // implicit drop at the end of this closure: `drain_available_lines`
            // (`crates/frust-cli/src/commands/run.rs`) treats a `Disconnected`
            // buffer as "the child is already reaped", then calls
            // `StreamHandle::wait`, which must never block. Stdout hitting EOF
            // well before the process is actually reaped (e.g. a child that
            // closes its own stdout early but keeps running) must NOT close
            // the buffer early, or that `wait` call deadlocks.
            shared_producer.close();
            success
        });

        let kill_child = Arc::clone(&child);
        // Unix: group-kill first so the whole process group dies (`cargo run`
        // + the preview binary it forks), then reap the direct child through
        // std for pid-reuse-safe cleanup. Non-Unix keeps today's
        // direct-child-only `Child::kill` (the tracked Windows job-object gap).
        #[cfg(unix)]
        let kill_action: Box<dyn FnOnce() + Send> = Box::new(move || {
            group_kill_unix(child_pid);
            let _ = kill_child
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .kill();
        });
        #[cfg(not(unix))]
        let kill_action: Box<dyn FnOnce() + Send> = Box::new(move || {
            let _ = kill_child
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .kill();
        });

        Ok(StreamHandle {
            lines: LineReceiver { shared },
            kill_action: Some(kill_action),
            worker: Some(worker),
            result: None,
        })
    }
}

/// A canned response for [`FakeProcessRunner`].
#[cfg(any(test, feature = "test-util"))]
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

/// A scripted [`ProcessRunner::spawn_streaming`] response for
/// [`FakeProcessRunner`].
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Clone)]
struct ScriptedStream {
    /// Each line to send, paired with how long to sleep before sending it
    /// (`Duration::ZERO` for no artificial delay).
    lines: Vec<(String, Duration)>,
    exit: StreamExit,
}

/// How a [`ScriptedStream`] finishes after its scripted lines are sent.
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Clone, Copy)]
enum StreamExit {
    /// Exits immediately with this success flag.
    Exit(bool),
    /// Never exits on its own — blocks until [`StreamHandle::kill`] is
    /// called. Used to test cancellation.
    Hang,
}

/// A fake [`ProcessRunner`] for tests: register canned responses keyed by
/// `"<cmd> <args...>"`. Lookup tries an exact match first, then falls back to
/// the longest registered key that is a prefix of the full invocation — this
/// lets a single registration match invocations with a dynamic trailing
/// argument (e.g. a temp-file path for `devicectl --json-output`).
#[cfg(any(test, feature = "test-util"))]
#[derive(Default)]
pub struct FakeProcessRunner {
    responses: HashMap<String, FakeOutcome>,
    streams: HashMap<String, ScriptedStream>,
    /// The `cwd` passed to the most recent `run_streaming`/`spawn_streaming`
    /// call, if any — see [`recorded_cwd`](Self::recorded_cwd). `Mutex`-backed
    /// (not a plain `Cell`/`RefCell`) because a `FakeProcessRunner` is
    /// routinely shared as `Arc<dyn ProcessRunner>` across a `spawn_blocking`
    /// boundary (e.g. `frust-tui`'s ad-hoc clean/build sessions), which
    /// requires `Sync`.
    recorded_cwd: Mutex<Option<PathBuf>>,
}

#[cfg(any(test, feature = "test-util"))]
impl FakeProcessRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a scripted [`spawn_streaming`](ProcessRunner::spawn_streaming)
    /// response: `lines` are sent in order with no artificial delay, then the
    /// fake stream exits with `success`.
    pub fn with_stream(
        mut self,
        key: impl Into<String>,
        lines: impl IntoIterator<Item = impl Into<String>>,
        success: bool,
    ) -> Self {
        self.streams.insert(
            key.into(),
            ScriptedStream {
                lines: lines
                    .into_iter()
                    .map(|line| (line.into(), Duration::ZERO))
                    .collect(),
                exit: StreamExit::Exit(success),
            },
        );
        self
    }

    /// Like [`with_stream`](Self::with_stream), but each line sleeps for its
    /// paired `Duration` before being sent — lets a test observe lines
    /// arriving over time instead of all at once.
    pub fn with_stream_delayed(
        mut self,
        key: impl Into<String>,
        lines: impl IntoIterator<Item = (impl Into<String>, Duration)>,
        success: bool,
    ) -> Self {
        self.streams.insert(
            key.into(),
            ScriptedStream {
                lines: lines
                    .into_iter()
                    .map(|(line, delay)| (line.into(), delay))
                    .collect(),
                exit: StreamExit::Exit(success),
            },
        );
        self
    }

    /// Register a scripted stream that sends `lines` then hangs — never
    /// exits on its own — until [`StreamHandle::kill`] is called. Used to
    /// test cancellation.
    pub fn with_hanging_stream(
        mut self,
        key: impl Into<String>,
        lines: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.streams.insert(
            key.into(),
            ScriptedStream {
                lines: lines
                    .into_iter()
                    .map(|line| (line.into(), Duration::ZERO))
                    .collect(),
                exit: StreamExit::Hang,
            },
        );
        self
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

    /// The `cwd` argument passed to the most recent
    /// [`run_streaming`](ProcessRunner::run_streaming)/
    /// [`spawn_streaming`](ProcessRunner::spawn_streaming) call, `None` if
    /// none has been made yet (or the caller passed `None`). Lets a test
    /// assert a caller actually routed a project-dir argument through rather
    /// than relying on the process's own current directory.
    pub fn recorded_cwd(&self) -> Option<PathBuf> {
        self.recorded_cwd
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Records `cwd` for [`recorded_cwd`](Self::recorded_cwd) — called by both
    /// `run_streaming` and `spawn_streaming` below.
    fn record_cwd(&self, cwd: Option<&Path>) {
        *self.recorded_cwd.lock().unwrap_or_else(|p| p.into_inner()) = cwd.map(Path::to_path_buf);
    }
}

#[cfg(any(test, feature = "test-util"))]
fn invocation_key(cmd: &str, args: &[&str]) -> String {
    std::iter::once(cmd)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(any(test, feature = "test-util"))]
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
    /// `on_line` before returning the same [`Output`]. `cwd` is recorded (see
    /// [`recorded_cwd`](Self::recorded_cwd)) but doesn't affect matching;
    /// `env` is ignored — invocation matching is keyed on `cmd`/`args` only,
    /// same as `run`.
    fn run_streaming(
        &self,
        cmd: &str,
        args: &[&str],
        cwd: Option<&Path>,
        _env: &[(&str, &str)],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<Output> {
        self.record_cwd(cwd);
        let out = self.run(cmd, args)?;
        for line in out.stdout.lines() {
            on_line(line);
        }
        Ok(out)
    }

    /// Fakes `spawn_streaming` from a registered [`with_stream`](FakeProcessRunner::with_stream)/
    /// [`with_stream_delayed`](FakeProcessRunner::with_stream_delayed)/
    /// [`with_hanging_stream`](FakeProcessRunner::with_hanging_stream) script,
    /// keyed on exact `"<cmd> <args...>"` (no prefix fallback, unlike `run`).
    /// `cwd` is recorded (see [`recorded_cwd`](Self::recorded_cwd)) but
    /// doesn't affect matching. A background thread sends the scripted lines
    /// (respecting any per-line delay), then exits per the script's
    /// [`StreamExit`] — `Hang` blocks on a real channel `recv`, so
    /// [`StreamHandle::kill`] unblocks it immediately, no polling.
    fn spawn_streaming(
        &self,
        cmd: &str,
        args: &[&str],
        cwd: Option<&Path>,
        _env: &[(&str, &str)],
    ) -> Result<StreamHandle> {
        self.record_cwd(cwd);
        let full = invocation_key(cmd, args);
        let scripted = self
            .streams
            .get(&full)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No such file or directory (os error 2): {cmd}"))?;

        let shared = LineBufferShared::new();
        let shared_producer = Arc::clone(&shared);
        let (kill_tx, kill_rx) = mpsc::channel::<()>();

        let worker = thread::spawn(move || {
            for (line, delay) in scripted.lines {
                if !delay.is_zero() {
                    thread::sleep(delay);
                }
                // Bounded, drop-oldest push — mirrors the real runner's
                // producer, which never blocks either (see
                // `LineBufferShared::push`).
                shared_producer.push(line);
            }
            let success = match scripted.exit {
                StreamExit::Exit(success) => success,
                StreamExit::Hang => {
                    // Blocks until `kill` sends, then reports "not
                    // successful" — mirroring a killed real process.
                    let _ = kill_rx.recv();
                    false
                }
            };
            // Close only now — not right after the scripted lines are sent —
            // mirroring the real runner's timing (see its own close-site
            // comment): a `Hang` script must keep the buffer open (so
            // `drain_available_lines` never sees a premature `Disconnected`
            // and calls `StreamHandle::wait` before `kill` has run) until the
            // fake process has actually "exited".
            shared_producer.close();
            success
        });

        Ok(StreamHandle {
            lines: LineReceiver { shared },
            kill_action: Some(Box::new(move || {
                let _ = kill_tx.send(());
            })),
            worker: Some(worker),
            result: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_lines_short_input_returns_all_lines() {
        let input = "line one\nline two\nline three";
        assert_eq!(tail_lines(input, 50), "line one\nline two\nline three");
    }

    #[test]
    fn tail_lines_long_input_returns_last_n_lines() {
        let input = (1..=100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = tail_lines(&input, 50);
        let tail_lines_vec: Vec<&str> = tail.lines().collect();
        assert_eq!(tail_lines_vec.len(), 50);
        assert_eq!(tail_lines_vec.first(), Some(&"line 51"));
        assert_eq!(tail_lines_vec.last(), Some(&"line 100"));
    }

    #[test]
    fn tail_lines_empty_input_returns_empty_string() {
        assert_eq!(tail_lines("", 50), "");
        assert_eq!(tail_lines("   \n\n  ", 50), "");
    }

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
        let tmp = std::env::temp_dir().join("frust-test-devicectl-prefix-match.json");
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

    #[test]
    fn spawn_streaming_fake_lines_arrive_in_order_then_exit_status_surfaces() {
        let runner =
            FakeProcessRunner::new().with_stream("adb logcat", ["line one", "line two"], true);
        let mut handle = runner
            .spawn_streaming("adb", &["logcat"], None, &[])
            .unwrap();

        let seen: Vec<String> = handle.lines.iter().collect();
        assert_eq!(seen, vec!["line one", "line two"]);
        assert!(handle.wait());
        // `wait` is safe to call again; cached result is returned.
        assert!(handle.wait());
    }

    #[test]
    fn spawn_streaming_fake_failed_exit_surfaces_as_unsuccessful() {
        let runner = FakeProcessRunner::new().with_stream("cmd fail", ["oops"], false);
        let mut handle = runner.spawn_streaming("cmd", &["fail"], None, &[]).unwrap();
        let seen: Vec<String> = handle.lines.iter().collect();
        assert_eq!(seen, vec!["oops"]);
        assert!(!handle.wait());
    }

    #[test]
    fn spawn_streaming_fake_kill_mid_hang_terminates_promptly_and_joins() {
        let runner = FakeProcessRunner::new().with_hanging_stream("adb logcat", ["first line"]);
        let mut handle = runner
            .spawn_streaming("adb", &["logcat"], None, &[])
            .unwrap();

        // Drain the one scripted line before the hang.
        assert_eq!(handle.lines.recv().as_deref(), Ok("first line"));

        let start = std::time::Instant::now();
        handle.kill();
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "kill() on a hung fake stream should unblock ~immediately, took {:?}",
            start.elapsed()
        );
        // A killed stream reports unsuccessful, mirroring a killed real process.
        assert!(!handle.wait());
    }

    #[test]
    fn spawn_streaming_fake_double_kill_is_a_noop() {
        let runner = FakeProcessRunner::new().with_hanging_stream("adb logcat", Vec::<&str>::new());
        let mut handle = runner
            .spawn_streaming("adb", &["logcat"], None, &[])
            .unwrap();
        handle.kill();
        let first = handle.wait();
        // A second kill after the worker has already been joined must not
        // panic and must not change the cached result.
        handle.kill();
        assert_eq!(handle.wait(), first);
    }

    #[test]
    fn spawn_streaming_fake_unregistered_invocation_errs() {
        let runner = FakeProcessRunner::new();
        assert!(
            runner
                .spawn_streaming("adb", &["logcat"], None, &[])
                .is_err()
        );
    }

    #[test]
    fn spawn_streaming_fake_drop_without_kill_does_not_block() {
        // Dropping a `StreamHandle` for a stream still hanging must not
        // block the caller (detach, not kill-on-drop — see the type's doc
        // comment). If this ever regressed to a join-on-drop, this test
        // would hang instead of returning.
        let runner = FakeProcessRunner::new().with_hanging_stream("adb logcat", ["one line"]);
        let handle = runner
            .spawn_streaming("adb", &["logcat"], None, &[])
            .unwrap();
        drop(handle);
    }

    #[test]
    fn line_buffer_cap_enforced_and_oldest_lines_dropped() {
        // Push `LINE_BUFFER_CAP + 5` lines directly at the ring buffer (no
        // process/thread involved — this is the buffer's own bookkeeping,
        // exercised in isolation for speed and determinism).
        let shared = LineBufferShared::new();
        for i in 0..(LINE_BUFFER_CAP + 5) {
            shared.push(format!("line {i}"));
        }
        shared.close();
        let receiver = LineReceiver {
            shared: Arc::clone(&shared),
        };

        let seen: Vec<String> = receiver.iter().collect();
        assert_eq!(seen.len(), LINE_BUFFER_CAP, "buffer must stay capped");
        // The oldest 5 lines (0..5) were dropped; the buffer starts at line 5.
        assert_eq!(seen.first(), Some(&"line 5".to_string()));
        assert_eq!(seen.last(), Some(&format!("line {}", LINE_BUFFER_CAP + 4)));
    }

    #[test]
    fn line_buffer_dropped_lines_counter_is_accurate() {
        let shared = LineBufferShared::new();
        let overflow = 37;
        for i in 0..(LINE_BUFFER_CAP + overflow) {
            shared.push(format!("line {i}"));
        }
        let receiver = LineReceiver { shared };
        assert_eq!(receiver.dropped_lines(), overflow as u64);
    }

    #[test]
    fn line_buffer_producer_never_blocks_even_far_past_capacity() {
        // A push at (or well past) capacity must never block the producer —
        // this is the no-deadlock guarantee `StreamHandle::kill` depends on
        // (see `LineBufferShared::push`'s doc). Bound the whole loop's
        // wall-clock time as a regression tripwire: a blocking push would
        // hang this test instead of finishing near-instantly.
        let shared = LineBufferShared::new();
        let start = std::time::Instant::now();
        for i in 0..(LINE_BUFFER_CAP * 2) {
            shared.push(format!("line {i}"));
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "pushing 2x capacity should never block; took {:?}",
            start.elapsed()
        );
    }

    /// Acceptance criterion: `kill()` must terminate promptly even while the
    /// stream's ring buffer sits at/over [`LINE_BUFFER_CAP`] and has never
    /// been drained — proving the drop-oldest push (not a bounded, blocking
    /// channel) really is what backs `spawn_streaming`.
    #[test]
    fn spawn_streaming_fake_kill_under_full_buffer_terminates_promptly() {
        let lines: Vec<String> = (0..(LINE_BUFFER_CAP + 20))
            .map(|i| format!("line {i}"))
            .collect();
        let runner = FakeProcessRunner::new().with_hanging_stream("adb logcat", lines);
        let mut handle = runner
            .spawn_streaming("adb", &["logcat"], None, &[])
            .unwrap();

        // Deliberately never drain `handle.lines` before killing — the
        // buffer fills past capacity and starts dropping while this thread
        // is elsewhere, exactly the scenario the no-deadlock guarantee
        // covers.
        let start = std::time::Instant::now();
        handle.kill();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "kill() must not wait on a full/overflowing buffer, took {:?}",
            start.elapsed()
        );
        assert!(!handle.wait());
    }

    #[test]
    fn spawn_streaming_real_process_lines_arrive_in_order_and_exit_status_surfaces() {
        let runner = RealProcessRunner;
        let mut handle = runner
            .spawn_streaming("/bin/sh", &["-c", "echo one; echo two"], None, &[])
            .unwrap();
        let seen: Vec<String> = handle.lines.iter().collect();
        assert_eq!(seen, vec!["one", "two"]);
        assert!(handle.wait());
    }

    #[test]
    fn spawn_streaming_real_kill_terminates_a_hung_process_promptly() {
        let runner = RealProcessRunner;
        let mut handle = runner
            .spawn_streaming("/bin/sh", &["-c", "sleep 30"], None, &[])
            .unwrap();

        let start = std::time::Instant::now();
        handle.kill();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "kill() should terminate the hung child well before its 30s sleep, took {elapsed:?}"
        );
        // A killed process is not "successful".
        assert!(!handle.wait());
    }

    /// Regression for the orphaned-preview bug: `kill()` must terminate the
    /// whole process *group*, not just the direct child. Mirrors `cargo run`
    /// forking the compiled preview binary — the shell (direct child) forks a
    /// `sleep` (grandchild) into the background, records its pid, then blocks
    /// in `wait`. Group-killing the shell must also reap the grandchild;
    /// before this fix a bare `Child::kill` left it (the preview window) alive.
    #[cfg(unix)]
    #[test]
    fn spawn_streaming_real_kill_terminates_the_whole_process_group() {
        // Unique tmp path for the grandchild-pid handoff (mktemp-style: pid +
        // a nanosecond nonce so parallel test runs never collide).
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pid_file = std::env::temp_dir().join(format!(
            "frust-grandchild-pid-{}-{nonce}.tmp",
            std::process::id()
        ));
        let pid_file_str = pid_file.to_string_lossy().to_string();

        let script = format!("sleep 12345 & echo $! > {pid_file_str}; wait");
        let runner = RealProcessRunner;
        let mut handle = runner
            .spawn_streaming("/bin/sh", &["-c", &script], None, &[])
            .unwrap();

        // Bounded poll (no fixed sleep) for the grandchild pid to be written.
        let grandchild_pid = read_pid_when_ready(&pid_file, Duration::from_secs(5))
            .expect("grandchild pid file should be written within the timeout");

        // Sanity: the grandchild is alive before the kill.
        assert!(
            !grandchild_is_dead(grandchild_pid),
            "grandchild (pid {grandchild_pid}) should be alive before the kill"
        );

        handle.kill();

        // Group-kill must reach the grandchild: assert it is gone within a
        // bounded poll (it reparents to init and is reaped once its parent
        // shell dies, so there is a brief settle window).
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut dead = false;
        while std::time::Instant::now() < deadline {
            if grandchild_is_dead(grandchild_pid) {
                dead = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = std::fs::remove_file(&pid_file);
        assert!(
            dead,
            "grandchild (pid {grandchild_pid}) still alive 2s after group kill — \
             kill did not reach the process group"
        );
    }

    /// Bounded-poll read of a pid written to `path` by a child shell, returning
    /// the parsed pid once the file exists and holds a full integer (avoids a
    /// fixed sleep racing the shell's `echo $!`).
    #[cfg(unix)]
    fn read_pid_when_ready(path: &Path, timeout: Duration) -> Option<i32> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if let Ok(contents) = std::fs::read_to_string(path)
                && let Ok(pid) = contents.trim().parse::<i32>()
            {
                return Some(pid);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// Probes whether `pid` is gone via `kill(pid, 0)` (delivers no signal,
    /// only checks existence/permission): dead iff the call fails with `ESRCH`.
    #[cfg(unix)]
    fn grandchild_is_dead(pid: i32) -> bool {
        // ESRCH ("no such process") is 3 on Linux and macOS.
        const ESRCH: i32 = 3;
        // # Safety: same contract as `group_kill_unix` — `kill` touches no
        // caller memory; signal `0` only probes the target without signalling.
        let rc = unsafe { kill(pid, 0) };
        rc == -1 && std::io::Error::last_os_error().raw_os_error() == Some(ESRCH)
    }

    /// The stdio-config invariant, asserted in one place: every TUI-reachable
    /// streaming spawn must pipe BOTH stdout and stderr — never inherit either,
    /// which would write the child's raw bytes onto the parent's raw-mode
    /// `frust tui` terminal and garble the whole screen (the LF-without-CR
    /// staircase). `run_streaming`/`spawn_streaming` both route through
    /// [`apply_streaming_stdio`], so pinning [`STREAMING_STDIO`] here pins both.
    #[test]
    fn streaming_spawns_pipe_both_streams() {
        // Compile-time invariant (a `const` block so a regression to
        // `Stdio::inherit()` — flipping either flag — fails to build, not just
        // at test time): every streaming spawn pipes BOTH streams, so no child
        // byte leaks to the raw-mode TUI tty.
        const {
            assert!(
                STREAMING_STDIO.stdout_piped,
                "a streaming spawn must PIPE stdout — inheriting it leaks child output to the tty"
            );
            assert!(
                STREAMING_STDIO.stderr_piped,
                "a streaming spawn must PIPE stderr — inheriting it leaks child output to the tty"
            );
        }
    }

    /// End-to-end proof (real child, no fake) that `run_streaming` pipes BOTH
    /// streams: stdout arrives through the line callback and stderr is captured
    /// into `Output.stderr`. If either were inherited the byte would go to the
    /// test process's own tty instead of being captured here.
    #[cfg(unix)]
    #[test]
    fn run_streaming_real_pipes_both_stdout_and_stderr() {
        let runner = RealProcessRunner;
        let mut lines = Vec::new();
        let out = runner
            .run_streaming(
                "/bin/sh",
                &["-c", "echo to-stdout; echo to-stderr 1>&2"],
                None,
                &[],
                &mut |line| lines.push(line.to_string()),
            )
            .unwrap();
        assert!(out.success);
        assert!(
            lines.iter().any(|l| l == "to-stdout"),
            "stdout should stream through on_line (piped), got {lines:?}"
        );
        assert!(
            out.stderr.contains("to-stderr"),
            "stderr should be captured into Output.stderr (piped, not inherited), got {:?}",
            out.stderr
        );
    }

    /// End-to-end proof (real child) that `spawn_streaming` pipes AND merges
    /// stderr into the line stream: a child writing to both stdout and stderr
    /// surfaces both as lines through the `StreamHandle`. Before the tty-leak
    /// fix stderr was piped but silently discarded; now it is merged.
    #[cfg(unix)]
    #[test]
    fn spawn_streaming_real_merges_stderr_into_the_line_stream() {
        let runner = RealProcessRunner;
        let mut handle = runner
            .spawn_streaming(
                "/bin/sh",
                &["-c", "echo to-stdout; echo to-stderr 1>&2"],
                None,
                &[],
            )
            .unwrap();
        let mut seen: Vec<String> = handle.lines.iter().collect();
        handle.wait();
        seen.sort();
        assert!(
            seen.iter().any(|l| l == "to-stdout"),
            "stdout should arrive as a line (piped), got {seen:?}"
        );
        assert!(
            seen.iter().any(|l| l == "to-stderr"),
            "stderr should be MERGED into the line stream (piped, not discarded), got {seen:?}"
        );
    }
}
