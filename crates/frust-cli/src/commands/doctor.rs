use std::path::Path;

use anyhow::{Context, Result};
use frust_drive::doctor::report::ComponentStatus;
use frust_drive::doctor::{DoctorCtx, RealEnv, Status, Validation, Validator};
use frust_drive::process::ProcessRunner;
use frust_drive::web_build::{self, WebPreflight};

/// Runs all doctor validators and prints their results, then the browser
/// preflight under its own "Web" heading, and returns the process exit code
/// by the one rule [`exit_code`] states: `1` if any validator is `Fail`, else
/// `0`. The browser checks ([`web_build::preflight`]) are informational only
/// and never affect the exit code — a host without wasm-bindgen or a
/// directory without a frust path dependency is not a doctor failure.
///
/// This entry point is where every host-bound input is fixed: the process
/// runner injected by `commands::dispatch` (the CLI's one `Real` construction
/// site), the [`RealEnv`] lookup seam, the host's own `target_os`, the
/// current directory, and [`frust_drive::doctor::default_validators`]. All
/// of them are parameters of [`run_with`], which is why the exit-code rule
/// can be tested against a scripted environment rather than against
/// whatever toolchains the machine running the test happens to have.
///
/// The browser checks are appended under their own "Web" heading rather than
/// folded into `default_validators()`'s flat list:
/// `frust-drive::doctor::report::Area` has no `Web` grouping of its own (the
/// component-level `DoctorReport`/`build_report` this crate never touches —
/// see that module's doc comment), so this is a CLI-side heading over the
/// same [`frust_drive::doctor::report::Component`] rows the browser pipeline
/// itself reports through, not a new `frust-drive` area. Each Web row prints
/// the preflight's own icon and summary (`[✗]` for a missing wasm-bindgen,
/// `[!]` for an absent wasm-opt) exactly as the browser pipeline reports it:
/// the rows are excluded from the exit code, not reworded.
pub fn run_in(runner: &dyn ProcessRunner, verbose: bool) -> Result<u8> {
    let env = RealEnv;
    let ctx = DoctorCtx {
        runner,
        env: &env,
        is_macos: cfg!(target_os = "macos"),
    };
    // The browser preflight is project-aware (it reports the resolved host
    // page and `[web]` manifest section), so it runs against the current
    // directory the same way `frust build`/`frust run` resolve their own
    // project root — a project-less directory still reports every host-tool
    // row honestly, just with the project-specific rows degraded rather
    // than failed (see `web_build::preflight`'s own doc comment).
    let cwd = std::env::current_dir().context("reading current directory")?;
    let validators = frust_drive::doctor::default_validators();
    Ok(run_with(&ctx, &validators, &cwd, verbose))
}

/// The testable core of [`run_in`]: the validator set, the context they run
/// under (process runner, env lookup, host OS) and the project directory the
/// browser preflight inspects are all parameters, so nothing about the
/// machine running a test leaks into the exit code it asserts.
fn run_with(
    ctx: &DoctorCtx<'_>,
    validators: &[Box<dyn Validator>],
    project_dir: &Path,
    verbose: bool,
) -> u8 {
    let results = frust_drive::doctor::run_all(ctx, validators);
    print_results(&results, verbose);

    let web_preflight = web_build::preflight(ctx.runner, project_dir);
    print_web_results(&web_preflight, verbose);

    exit_code(&results)
}

/// The exit-code rule in one place: `1` if any validator is `Fail`, else
/// `0`. Deliberately takes only the validator results — the browser
/// preflight is printed by [`run_with`] but never consulted here, which is
/// what makes its rows informational.
fn exit_code(results: &[(String, Validation)]) -> u8 {
    let any_fail = results
        .iter()
        .any(|(_, validation)| validation.status == Status::Fail);
    if any_fail { 1 } else { 0 }
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
    use frust_drive::doctor::EnvLookup;
    use frust_drive::process::{FakeProcessRunner, Output};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// An env lookup that knows no variable at all — no `ANDROID_HOME`, no
    /// `CARGO_TARGET_DIR` — so nothing this machine exports reaches a
    /// validator under test.
    struct EmptyEnv;

    impl EnvLookup for EmptyEnv {
        fn get(&self, _key: &str) -> Option<String> {
            None
        }
    }

    /// A validator with a fixed answer, standing in for the real set so the
    /// exit-code rule is established against a known input rather than
    /// against whichever toolchains the test host has installed.
    struct Fixed(Status);

    impl Validator for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }

        fn validate(&self, _ctx: &DoctorCtx) -> Validation {
            Validation {
                status: self.0,
                messages: vec!["scripted".to_string()],
            }
        }
    }

    fn fixed(statuses: &[Status]) -> Vec<Box<dyn Validator>> {
        statuses
            .iter()
            .map(|status| Box::new(Fixed(*status)) as Box<dyn Validator>)
            .collect()
    }

    fn ctx<'a>(runner: &'a FakeProcessRunner, env: &'a EmptyEnv) -> DoctorCtx<'a> {
        DoctorCtx {
            runner,
            env,
            is_macos: false,
        }
    }

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    /// A fresh directory that is not a project (no `Cargo.toml`), so the
    /// browser preflight's project rows degrade the way `frust doctor` in an
    /// arbitrary directory sees them.
    fn empty_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("frust-cli-doctor-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn row(status: Status) -> (String, Validation) {
        (
            "row".to_string(),
            Validation {
                status,
                messages: Vec::new(),
            },
        )
    }

    #[test]
    fn exit_code_is_one_iff_a_validator_fails() {
        assert_eq!(exit_code(&[]), 0);
        assert_eq!(exit_code(&[row(Status::Pass), row(Status::Partial)]), 0);
        assert_eq!(exit_code(&[row(Status::Pass), row(Status::Fail)]), 1);
    }

    /// The contract in one scripted run: a browser preflight that is not
    /// ready (an empty runner knows no `rustup` and no `wasm-bindgen`, and
    /// the directory is no project) leaves the exit code at 0 when every
    /// validator passes — the Web rows are printed, never counted.
    #[test]
    fn a_not_ready_web_preflight_never_changes_the_exit_code() {
        let dir = empty_dir("not-ready");
        let runner = FakeProcessRunner::new();
        let env = EmptyEnv;
        assert!(
            !web_build::preflight(&runner, &dir).is_ready(),
            "precondition: the preflight under test must be degraded"
        );
        let validators = fixed(&[Status::Pass, Status::Partial]);
        assert_eq!(run_with(&ctx(&runner, &env), &validators, &dir, false), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other half of the same rule: a failing validator exits 1 whether
    /// or not the Web rows are healthy — the preflight cannot rescue a run
    /// any more than it can sink one.
    #[test]
    fn a_failing_validator_exits_one_regardless_of_the_web_rows() {
        let dir = empty_dir("validator-fail");
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("wasm32-unknown-unknown\n"),
            )
            .with("wasm-bindgen --version", ok("wasm-bindgen 0.2.128\n"))
            .with("wasm-opt --version", ok("wasm-opt version 130\n"));
        let env = EmptyEnv;
        let validators = fixed(&[Status::Pass, Status::Fail]);
        assert_eq!(run_with(&ctx(&runner, &env), &validators, &dir, false), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The delta form of the rule, on the row that first motivated it:
    /// the same validator set yields the same exit code with `wasm-bindgen`
    /// present and with it absent.
    #[test]
    fn the_wasm_bindgen_row_is_informational() {
        let dir = empty_dir("bindgen-delta");
        let env = EmptyEnv;
        let validators = fixed(&[Status::Pass]);
        let present =
            FakeProcessRunner::new().with("wasm-bindgen --version", ok("wasm-bindgen 0.2.128\n"));
        let absent = FakeProcessRunner::new().missing("wasm-bindgen --version");
        assert_eq!(
            run_with(&ctx(&present, &env), &validators, &dir, false),
            run_with(&ctx(&absent, &env), &validators, &dir, false),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The production entry point runs end to end on any host: the only error
    /// it can return is an unreadable current directory. The exit code is
    /// deliberately not asserted here — it depends on this machine's
    /// toolchains, which is exactly what the scripted tests above avoid.
    #[test]
    fn run_in_reaches_the_web_preflight() {
        run_in(&FakeProcessRunner::new(), false).expect("doctor runs to completion");
    }

    /// [`print_web_results`] never panics on an empty component list (a
    /// defensive shape check for the rendering helper itself, independent of
    /// whatever `web_build::preflight` reports on this host).
    #[test]
    fn print_web_results_handles_empty_components() {
        print_web_results(&WebPreflight { components: vec![] }, true);
    }
}
