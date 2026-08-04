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
/// Both streams gate deliberately, not stdout alone: stdout must be a TTY
/// since that is where the TUI's frames are drawn, but requiring stdin as
/// well is a deliberate conservative narrowing on top of that — it rules out
/// `frust | grep` (stdin still a TTY) launching into the interactive TUI and
/// leaking raw ANSI into the pipe, at the cost of also refusing a
/// stdin-redirected/stdout-TTY shape that would otherwise still work.
fn default_command(stdin_tty: bool, stdout_tty: bool) -> Option<Command> {
    (stdin_tty && stdout_tty).then_some(Command::Tui)
}

fn main() -> ExitCode {
    let mut cli = Cli::parse();
    if cli.command.is_none() {
        cli.command = default_command(
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
        );
    }
    if cli.command.is_none() {
        // Non-TTY stdio and no subcommand given: today's script-facing
        // contract is exit code 2, preserved from clap's own
        // missing-subcommand behavior. What deliberately changed: full help
        // prints to stdout here (clap's own missing-subcommand error instead
        // renders a short usage snippet to stderr), so stderr stays silent on
        // this branch.
        //
        // Every write below must tolerate failure (`let _ =`, never a bare
        // `println!`/`print!`): a closed or non-blocking stdout (e.g. `frust
        // | head -0`) surfaces as EPIPE, and Rust's default SIGPIPE handling
        // turns that into a panic (exit 101) rather than the write silently
        // failing — which would pre-empt the exit-2 contract this branch
        // exists to guarantee.
        let _ = Cli::command().print_help();
        let _ = writeln!(std::io::stdout());
        return ExitCode::from(2);
    }
    match commands::dispatch(cli) {
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
