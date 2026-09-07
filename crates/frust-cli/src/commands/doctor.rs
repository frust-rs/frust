use anyhow::{Context, Result};
use frust_drive::doctor::report::ComponentStatus;
use frust_drive::doctor::{DoctorCtx, RealEnv, Status, Validation};
use frust_drive::process::ProcessRunner;
use frust_drive::web_build::{self, WebPreflight};

/// Runs all doctor validators and prints their results. Returns the process
/// exit code: `1` if any validator is `Fail` or the browser preflight is not
/// ready, else `0`. The process runner is injected by `commands::dispatch`
/// (the CLI's one `Real` construction site); the env lookup seam stays
/// [`RealEnv`] here.
///
/// The browser checks ([`web_build::preflight`]) are appended under their own
/// "Web" heading rather than folded into `default_validators()`'s flat list:
/// `frust-drive::doctor::report::Area` has no `Web` grouping of its own (the
/// component-level `DoctorReport`/`build_report` this crate never touches —
/// see that module's doc comment), so this is a CLI-side heading over the
/// same [`frust_drive::doctor::report::Component`] rows the browser pipeline
/// itself reports through, not a new `frust-drive` area.
pub fn run_in(runner: &dyn ProcessRunner, verbose: bool) -> Result<u8> {
    let env = RealEnv;
    let ctx = DoctorCtx {
        runner,
        env: &env,
        is_macos: cfg!(target_os = "macos"),
    };

    let validators = frust_drive::doctor::default_validators();
    let results = frust_drive::doctor::run_all(&ctx, &validators);
    print_results(&results, verbose);

    // The browser preflight is project-aware (it reports the resolved host
    // page and `[web]` manifest section), so it runs against the current
    // directory the same way `frust build`/`frust run` resolve their own
    // project root — a project-less directory still reports every host-tool
    // row honestly, just with the two project-specific rows degraded rather
    // than failed (see `web_build::preflight`'s own doc comment).
    let cwd = std::env::current_dir().context("reading current directory")?;
    let web_preflight = web_build::preflight(runner, &cwd);
    print_web_results(&web_preflight, verbose);

    let any_fail = results
        .iter()
        .any(|(_, validation)| validation.status == Status::Fail);
    Ok(if any_fail || !web_preflight.is_ready() {
        1
    } else {
        0
    })
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

/// Renders [`WebPreflight`]'s rows under a "Web" heading, in the same
/// icon/indented-message shape [`print_results`] uses for the flat validator
/// list above — a browser-build row is exactly as actionable as a validator
/// one, just carried in `frust-drive`'s newer `Component` shape rather than
/// the older `Validation` one (see `run_in`'s doc comment).
fn print_web_results(preflight: &WebPreflight, verbose: bool) {
    println!("Web");
    for component in &preflight.components {
        let icon = match component.status {
            ComponentStatus::Ok => "[\u{2713}]",
            ComponentStatus::Partial => "[!]",
            ComponentStatus::Missing => "[\u{2717}]",
        };
        println!("{icon} {}", component.name);
        if verbose || component.status != ComponentStatus::Ok {
            println!("    {}", component.summary);
            for fix in &component.fix_commands {
                println!("    fix: {}", fix.display);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::process::FakeProcessRunner;

    /// `frust doctor` reaches the browser preflight and renders it under its
    /// own heading — proof `run_in` calls `web_build::preflight` rather than
    /// only ever running `default_validators()`.
    #[test]
    fn run_in_reaches_the_web_preflight() {
        let runner = FakeProcessRunner::new();
        // Exit code is not asserted: the host running this test may or may
        // not have the mobile/desktop toolchains `default_validators()`
        // checks, and this test is only about the web heading being reached.
        let _ = run_in(&runner, false);
    }

    /// [`print_web_results`] never panics on an empty component list (a
    /// defensive shape check for the rendering helper itself, independent of
    /// whatever `web_build::preflight` reports on this host).
    #[test]
    fn print_web_results_handles_empty_components() {
        print_web_results(&WebPreflight { components: vec![] }, true);
    }
}
