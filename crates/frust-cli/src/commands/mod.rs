//! Command dispatch.

pub mod build;
pub mod clean;
pub mod create;
pub mod dap;
pub mod devices;
pub mod doctor;
pub mod run;
pub mod tui;

use crate::cli::{Cli, Command};
use anyhow::{Context, Result};
use frust_drive::process::RealProcessRunner;

/// Runs the given subcommand, returning the process exit code.
///
/// The real [`RealProcessRunner`] is constructed here, in one place, and
/// injected into every drive-touching handler's `run_in` core (the CLI's
/// sole `Real` construction site); `create` takes no runner (it only writes
/// files).
///
/// Takes the already-resolved `Command` rather than `Cli` (whose `command`
/// field is `Option<Command>`): `main` resolves bare `frust`'s `None` (TUI
/// on a TTY, help + exit 2 otherwise) before ever calling `dispatch`, so the
/// resolved command arrives here as a plain `Command`, not an `Option` —
/// this is the single dispatch site for every `Command`, including the
/// bare-`frust` default. The rest of `cli` (`verbose`, `device_id`) is still
/// read from `&Cli`. Note `cli.command` has itself been `.take()`n by `main`
/// before this call and is always `None` inside `dispatch`; handlers must
/// read the `command` parameter, never `cli.command`.
pub fn dispatch(command: Command, cli: &Cli) -> Result<u8> {
    let verbose = cli.verbose > 0;
    let runner: std::sync::Arc<dyn frust_drive::process::ProcessRunner + Send + Sync> =
        std::sync::Arc::new(RealProcessRunner);
    match command {
        Command::Doctor => doctor::run_in(&*runner, verbose),
        Command::Devices => devices::run_in(&*runner, verbose),
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
            clean::run_in(&*runner, &cwd)
        }
        Command::Dap { port } => dap::run_in(runner.clone(), port),
        Command::Tui => tui::run(),
        Command::Run {
            build,
            render_tier,
            watch,
        } => run::run_in(
            &*runner,
            build,
            cli.device_id.clone(),
            render_tier,
            watch,
            verbose,
        ),
        Command::Build { target } => {
            let cwd = std::env::current_dir().context("reading current directory")?;
            build::run_in(&*runner, &cwd, target)
        }
    }
}
