//! The `frust` CLI binary.

mod build_args;
mod cli;
mod commands;

use clap::{CommandFactory, Parser};
use cli::{Cli, Command};
use std::io::IsTerminal;
use std::process::ExitCode;

/// Pure decision: what does bare `frust` (no subcommand) do? Testable
/// without a real TTY.
///
/// Both streams gate deliberately, not stdout alone: crossterm gates raw
/// mode on stdin, so a stdout-only check would let `frust | grep` (stdin
/// still a TTY) slip into raw mode and leak ANSI into the pipe.
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
        // contract (help text + exit 2), same shape clap's own
        // missing-subcommand error gives.
        let _ = Cli::command().print_help();
        println!();
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
