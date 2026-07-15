use crate::doctor::{self, DoctorCtx, RealEnv, Status, Validation};
use crate::process::RealProcessRunner;
use anyhow::Result;

/// Runs all doctor validators and prints their results. Returns the process
/// exit code: `1` if any validator is `Fail`, else `0`.
pub fn run(verbose: bool) -> Result<u8> {
    let runner = RealProcessRunner;
    let env = RealEnv;
    let ctx = DoctorCtx {
        runner: &runner,
        env: &env,
        is_macos: cfg!(target_os = "macos"),
    };

    let validators = doctor::default_validators();
    let results = doctor::run_all(&ctx, &validators);

    print_results(&results, verbose);

    let any_fail = results
        .iter()
        .any(|(_, validation)| validation.status == Status::Fail);
    Ok(if any_fail { 1 } else { 0 })
}

fn print_results(results: &[(String, Validation)], verbose: bool) {
    for (name, validation) in results {
        let icon = match validation.status {
            Status::Pass => "[\u{2713}]",
            Status::Partial => "[!]",
            Status::Fail => "[\u{2717}]",
        };
        println!("{icon} {name}");
        if verbose || validation.status != Status::Pass {
            for message in &validation.messages {
                println!("    {message}");
            }
        }
    }
}
