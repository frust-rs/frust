//! Command dispatch.

pub mod build;
pub mod clean;
pub mod create;
pub mod devices;
pub mod doctor;
pub mod run;
pub mod tui;

use crate::cli::{Cli, Command};
use anyhow::{Context, Result};
use frust_drive::process::RealProcessRunner;

/// Runs the selected subcommand, returning the process exit code.
///
/// The real [`RealProcessRunner`] is constructed here, in one place, and
/// injected into every drive-touching handler's `run_in` core (the CLI's
/// sole `Real` construction site); `create` takes no runner (it only writes
/// files).
///
/// `cli.command` must be `Some` by the time it reaches here — `main`
/// resolves bare `frust`'s `None` (TUI on a TTY, help + exit 2 otherwise)
/// before ever calling `dispatch`, so this is the single dispatch site for
/// every `Command`, including the bare-`frust` default.
pub fn dispatch(cli: Cli) -> Result<u8> {
    let verbose = cli.verbose > 0;
    let runner = RealProcessRunner;
    let command = cli
        .command
        .expect("main resolves `command` to `Some` before calling dispatch");
    match command {
        Command::Doctor => doctor::run_in(&runner, verbose),
        Command::Devices => devices::run_in(&runner, verbose),
        Command::Create {
            dir,
            org,
            project_name,
            description,
            overwrite,
            template_dir,
            frust_path,
            deeplink_scheme,
            deeplink_host,
            arch,
        } => create::run(create::CreateArgs {
            dir,
            org,
            project_name,
            description,
            overwrite,
            template_dir,
            frust_path,
            deeplink_scheme,
            deeplink_host,
            arch: arch.map(|a| a.as_str().to_string()),
        }),
        Command::Clean => {
            let cwd = std::env::current_dir().context("reading current directory")?;
            clean::run_in(&runner, &cwd)
        }
        Command::Tui => tui::run(),
        Command::Run {
            build,
            render_tier,
            watch,
        } => run::run_in(&runner, build, cli.device_id, render_tier, watch, verbose),
        Command::Build { target } => {
            let cwd = std::env::current_dir().context("reading current directory")?;
            build::run_in(&runner, &cwd, target)
        }
    }
}
