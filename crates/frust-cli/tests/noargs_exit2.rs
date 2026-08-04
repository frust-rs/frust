//! Pins the bare-`frust`-with-non-TTY-stdio contract against the real
//! binary: exit code 2, full help on stdout, silent stderr.
//!
//! Unlike `default_command`'s unit test (`src/main.rs`, exercised with hand-fed
//! booleans), this drives the compiled `frust` binary (`CARGO_BIN_EXE_frust`,
//! Cargo builds and points at it automatically for integration tests — see
//! `create_e2e.rs`/`interrupt_e2e.rs`) with every stream piped/redirected, so
//! `is_terminal()` reads false the way it would under CI or a script, no
//! hand-fed booleans standing in for a real non-interactive launch. Fast (no
//! compile-a-project step, unlike the `--ignored` e2e tests in this
//! directory), so it runs in the default `cargo test -p frust-cli` gate.

use std::io::Read;
use std::process::{Command, Stdio};

/// A `frust` invocation with colour forced off, for every spawn in this
/// file. clap's `anstream` auto-detects colour support from the
/// environment as well as from `is_terminal()`, so a `CLICOLOR_FORCE=1`
/// (or similarly colour-forcing) ambient environment — a real, reproduced
/// failure, not a hypothetical — makes clap render ANSI-escaped help,
/// breaking the plain-text `"Usage: frust"` assertion below even though
/// stdout is piped (non-TTY). Pinning `NO_COLOR` and clearing the
/// `CLICOLOR*` overrides makes every assertion in this file independent of
/// the caller's terminal/env-var state.
fn frust_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_frust"));
    command
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("CLICOLOR");
    command
}

/// Bare `frust`, no args, all three streams piped/redirected (guaranteed
/// non-TTY): must exit 2, print full help to stdout, and stay silent on
/// stderr.
#[test]
fn bare_frust_with_no_tty_exits_2_with_help_on_stdout_and_silent_stderr() {
    let mut child = frust_command()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `frust`");

    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .read_to_string(&mut stdout)
        .expect("reading stdout");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_string(&mut stderr)
        .expect("reading stderr");

    let status = child.wait().expect("waiting for `frust`");

    assert_eq!(
        status.code(),
        Some(2),
        "bare `frust` with non-TTY stdio must exit 2; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("Usage: frust"),
        "expected full help on stdout, got: {stdout:?}"
    );
    assert!(
        stderr.is_empty(),
        "this branch's contract is stdout-only help with silent stderr, got: {stderr:?}"
    );
}

/// `frust tui` with piped/redirected (non-TTY) stdio: the TUI guard's
/// clean-refusal path — `ensure_interactive_terminal` (`frust-tui`'s
/// `runner.rs`) must reject before `ratatui::init()` ever runs, exiting 1
/// with its own message on stderr.
///
/// Stdout is discarded (`Stdio::null()`, nothing reads it) rather than
/// piped-and-unread: if the guard ever regressed and `ratatui::init()` ran
/// anyway, a piped stdout no one reads would fill its OS pipe buffer and
/// hang this test forever the moment the TUI tried to draw a frame.
///
/// The assertions are pinned to the guard's own contract — exit code 1 and
/// its exact `"interactive terminal"` message on stderr — rather than the
/// looser "nonzero exit, nonempty stderr" this test used to assert: a
/// `ratatui::init()` panic (the very failure this guard exists to prevent)
/// also exits nonzero with nonempty stderr, so the looser assertions
/// couldn't actually distinguish the clean guard from a crash.
#[test]
fn explicit_tui_subcommand_with_no_tty_exits_1_with_guard_message_on_stderr() {
    let mut child = frust_command()
        .arg("tui")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `frust tui`");

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_string(&mut stderr)
        .expect("reading stderr");

    let status = child.wait().expect("waiting for `frust tui`");

    assert_eq!(
        status.code(),
        Some(1),
        "the TUI guard's clean-refusal path must exit 1; status={status:?} stderr={stderr:?}"
    );
    assert!(
        stderr.contains("interactive terminal"),
        "expected the TUI guard's own error message on stderr, got: {stderr:?}"
    );
}

/// `frust`'s exit-2 branch writes to stdout with `let _ =` rather than a
/// bare `println!`/`writeln!`, specifically so a closed stdout can't turn
/// into a SIGPIPE panic that pre-empts the exit-2 contract (see `main.rs`'s
/// doc comment on that branch). This test constructs exactly that failure
/// deterministically: a pipe whose read end is dropped *before* the child
/// ever spawns has no reader at all, so the child's first write to it fails
/// EPIPE immediately (Rust ignores `SIGPIPE`, so this surfaces as a plain
/// `Err`, not a signal-terminated process) — no timing race with a real
/// reader required.
///
/// By construction this would fail against the pre-hardening code shape (a
/// bare `println!`/`writeln!`): the write panics, and a panic during
/// `cargo test`'s debug build exits 101, not 2.
#[cfg(unix)]
#[test]
fn bare_frust_with_no_tty_and_closed_stdout_still_exits_2() -> std::io::Result<()> {
    let (reader, writer) = std::io::pipe()?;
    drop(reader);

    let mut child = frust_command()
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `frust`");

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_string(&mut stderr)
        .expect("reading stderr");

    let status = child.wait().expect("waiting for `frust`");

    assert_eq!(
        status.code(),
        Some(2),
        "a closed stdout must not pre-empt the exit-2 contract via a SIGPIPE panic; \
         status={status:?} stderr={stderr:?}"
    );
    assert!(
        stderr.is_empty(),
        "the exit-2 branch's contract is silent stderr even with a closed stdout, got: {stderr:?}"
    );

    Ok(())
}
