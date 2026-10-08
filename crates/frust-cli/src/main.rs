//! The `frust` CLI binary.

mod build_args;
mod cli;
mod commands;

use clap::{CommandFactory, Parser};
use cli::{Cli, Command};
use std::io::{IsTerminal, Write};
use std::process::ExitCode;

/// Pure decision: what does bare `frust` (no subcommand) do? Testable
/// without a real TTY.
///
/// Both streams gate deliberately, not stdout alone: stdout must be a TTY,
/// since that is where the TUI's frames (and this branch's help text) are
/// written — that term alone already rules out `frust | grep` (stdout is
/// the pipe, not a TTY, regardless of stdin). Requiring stdin as well is a
/// deliberate conservative narrowing on top of that; its sole effect is
/// refusing the stdin-redirected/stdout-TTY shape (`frust < file`), which
/// stdout-only gating would otherwise still let through. This predicate has
/// a twin, `frust-tui`'s `ensure_interactive_terminal`
/// (`crates/frust-tui/src/runner.rs`) — the two must move in lockstep.
fn default_command(stdin_tty: bool, stdout_tty: bool) -> Option<Command> {
    (stdin_tty && stdout_tty).then_some(Command::Tui)
}

/// `frust` as cargo's `RUSTC_WORKSPACE_WRAPPER` (and rustc's linker) for a
/// hot-patch fat build: record or intercept this one invocation and exit,
/// with no CLI parsing at all (`frust_drive::hotpatch::capture`).
fn run_hotpatch_wrapper(scope_dir: &std::path::Path) -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let envs: Vec<_> = std::env::vars_os().collect();
    let result = frust_drive::hotpatch::capture::run_wrapper(
        &frust_drive::process::RealProcessRunner,
        scope_dir,
        &args,
        &envs,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    );
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            let _ = writeln!(std::io::stderr(), "frust: {err}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    if let Some(scope_dir) = frust_drive::hotpatch::capture::wrapper_scope_from_env() {
        return run_hotpatch_wrapper(&scope_dir);
    }
    let mut cli = Cli::parse();
    if cli.command.is_none() {
        cli.command = default_command(
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
        );
    }
    let Some(command) = cli.command.take() else {
        // Non-TTY stdio and no subcommand given: today's script-facing
        // contract is exit code 2, preserved from clap's own
        // missing-subcommand behavior. For bare `frust`, the help CONTENT is
        // unchanged from clap's old behavior — clap_derive emits
        // `subcommand_required(true).arg_required_else_help(true)` for a
        // non-`Option` subcommand field, and with zero args present,
        // `arg_required_else_help` is evaluated before `missing_subcommand`
        // (clap_builder's `validator.rs` orders the two checks that way), so
        // clap already rendered this same full help; only the stream moved
        // (stderr -> stdout), and exit 2 is preserved. `frust <global-flag>`
        // with no subcommand (e.g. `frust -v`) is the shape that actually
        // changed: it previously got clap's own missing-subcommand error — a
        // short usage snippet — on stderr, and now gets this same full help
        // on stdout instead.
        //
        // Every write below must tolerate failure (`let _ =`, never a bare
        // `println!`/`print!`): a closed or non-blocking stdout (e.g. `frust
        // | head -0`) surfaces as EPIPE, and Rust's default SIGPIPE handling
        // turns that into a panic (exit 101 in a debug build, an abort under
        // the shipped `panic = "abort"` release profile) rather than the
        // write silently failing — which would pre-empt the exit-2 contract
        // this branch exists to guarantee.
        let _ = Cli::command().print_help();
        let _ = writeln!(std::io::stdout());
        return ExitCode::from(2);
    };
    match commands::dispatch(command, &cli) {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("Error: {err:#}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_command_only_tui_when_both_streams_are_terminals() {
        assert!(matches!(default_command(true, true), Some(Command::Tui)));
        assert!(default_command(true, false).is_none());
        assert!(default_command(false, true).is_none());
        assert!(default_command(false, false).is_none());
    }
}
