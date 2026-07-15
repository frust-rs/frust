//! The `forgekit` CLI binary (spec §12).

// `build_info` is scaffolding: `BuildInfo`/`BuildArgs` aren't wired into any
// command yet (spec Phase 2 `run`/`build` will consume them) so nothing in
// the non-test build tree constructs them yet — see task 04 notes.
#[allow(dead_code)]
mod build_info;
mod cli;
mod commands;
mod devices;
mod doctor;
mod process;
mod scaffold;

use clap::Parser;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match commands::dispatch(cli) {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("Error: {err:#}");
            ExitCode::from(1)
        }
    }
}
