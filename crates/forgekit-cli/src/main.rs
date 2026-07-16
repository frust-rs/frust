//! The `forgekit` CLI binary (spec §12).

mod android_build;
mod android_id;
mod android_run;
mod build_info;
mod cli;
mod commands;
mod devices;
mod doctor;
mod ios_build;
mod ios_id;
mod ios_run;
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
