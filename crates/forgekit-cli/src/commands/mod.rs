//! Command dispatch (spec §12.1).

pub mod build;
pub mod clean;
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
            deeplink_scheme,
            deeplink_host,
        } => create::run(create::CreateArgs {
            dir,
            org,
            project_name,
            description,
            overwrite,
            template_dir,
            forgekit_path,
            deeplink_scheme,
            deeplink_host,
        }),
        Command::Clean => clean::run(),
        Command::Run { build, render_tier } => run::run(build, cli.device_id, render_tier, verbose),
        Command::Build { target } => build::run(target),
    }
}
