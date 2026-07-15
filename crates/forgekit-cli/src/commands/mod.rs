//! Command dispatch (spec §12.1).

pub mod devices;
pub mod doctor;

use crate::cli::{Cli, Command};
use anyhow::{bail, Result};

/// Runs the selected subcommand, returning the process exit code.
pub fn dispatch(cli: Cli) -> Result<u8> {
    let verbose = cli.verbose > 0;
    match cli.command {
        Command::Doctor => doctor::run(verbose),
        Command::Devices => devices::run(verbose),
        Command::Create { .. } => bail!("`forgekit create` is not implemented yet — todo — task 05"),
        Command::Clean => {
            println!("`forgekit clean` is not implemented yet.");
            Ok(0)
        }
    }
}
