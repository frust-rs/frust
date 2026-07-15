//! Command dispatch (spec §12.1).

pub mod create;
pub mod devices;
pub mod doctor;
pub mod run;

use crate::cli::{Cli, Command};
use anyhow::Result;

/// Runs the selected subcommand, returning the process exit code.
pub fn dispatch(cli: Cli) -> Result<u8> {
    let verbose = cli.verbose > 0;
    match cli.command {
        Command::Doctor => doctor::run(verbose),
        Command::Devices => devices::run(verbose),
        Command::Create {
            dir,
            org,
            project_name,
            description,
            overwrite,
            template_dir,
            forgekit_path,
        } => create::run(create::CreateArgs {
            dir,
            org,
            project_name,
            description,
            overwrite,
            template_dir,
            forgekit_path,
        }),
        Command::Clean => {
            println!("`forgekit clean` is not implemented yet.");
            Ok(0)
        }
        Command::Run { build } => run::run(build, cli.device_id, verbose),
    }
}
