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

/// Bare `frust`, no args, all three streams piped (guaranteed non-TTY):
/// must exit 2, print full help to stdout, and stay silent on stderr.
#[test]
fn bare_frust_with_no_tty_exits_2_with_help_on_stdout_and_silent_stderr() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_frust"))
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

/// `frust tui` with piped (non-TTY) stdio: the TUI guard's clean-refusal
/// path — a typed error on stderr and a nonzero exit, never a panic from
/// `ratatui::init()` or raw ANSI leaking into the pipe.
#[test]
fn explicit_tui_subcommand_with_no_tty_exits_nonzero_with_error_on_stderr() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_frust"))
        .arg("tui")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
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

    assert!(
        !status.success(),
        "`frust tui` with non-TTY stdio must fail, not silently succeed"
    );
    assert!(
        !stderr.is_empty(),
        "the TUI guard's clean-refusal path must report an error on stderr"
    );
}
