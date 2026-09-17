//! `wasm32-unknown-unknown` presence check for the browser pipeline
//! (`crate::web_build`).
//!
//! **Non-fatal by design**, like [`super::CargoPackagerValidator`]: a browser
//! build is opt-in on every host, so a missing target is a
//! [`Status::Partial`] here and never trips `commands::doctor::run_in`'s
//! exit-1 gate. `crate::web_build::preflight` reads the *same* [`probe`] and
//! reaches the opposite verdict for its own row — once a browser build has
//! actually been asked for, the target is the one thing it cannot proceed
//! without — which is why the classification lives in [`TargetProbe`] and the
//! severity lives with each caller.

use super::{DoctorCtx, Status, Validation, Validator};
use crate::build_info::WASM_TARGET_TRIPLE;
use crate::process::ProcessRunner;

/// What `rustup target list --installed` says about the wasm32 target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetProbe {
    Installed,
    NotInstalled,
    /// `rustup` itself could not be run, so nothing is known about the
    /// installed set — distinct from a known-absent target, because the fix
    /// is to install `rustup`, not to add a target to it.
    RustupUnavailable,
}

impl TargetProbe {
    /// The wording both the flat doctor row and
    /// `web_build::preflight`'s component render, so the two surfaces can
    /// never describe the same host differently.
    pub(crate) fn summary(self) -> String {
        match self {
            Self::Installed => format!("{WASM_TARGET_TRIPLE} installed"),
            Self::NotInstalled => {
                format!("{WASM_TARGET_TRIPLE} is not installed on the active toolchain")
            }
            Self::RustupUnavailable => {
                "rustup could not be run, so the installed targets are unknown".to_string()
            }
        }
    }
}

/// The command that adds the target, as a human string. The structured form
/// of the same fix is `web_build::preflight`'s/`doctor::report`'s
/// `FixCommand`.
pub(crate) fn add_target_command() -> String {
    format!("rustup target add {WASM_TARGET_TRIPLE}")
}

/// The single wasm32-target probe: one `rustup` invocation, one
/// classification, shared by [`WasmTargetValidator`] and
/// `web_build::preflight`'s target row.
pub(crate) fn probe(runner: &dyn ProcessRunner) -> TargetProbe {
    let installed = match runner.run("rustup", &["target", "list", "--installed"]) {
        Ok(out) if out.success => out.stdout,
        _ => return TargetProbe::RustupUnavailable,
    };
    if installed
        .lines()
        .any(|line| line.trim() == WASM_TARGET_TRIPLE)
    {
        TargetProbe::Installed
    } else {
        TargetProbe::NotInstalled
    }
}

pub struct WasmTargetValidator;

impl Validator for WasmTargetValidator {
    fn name(&self) -> &str {
        "wasm32 target"
    }

    fn validate(&self, ctx: &DoctorCtx) -> Validation {
        let probe = probe(ctx.runner);
        let summary = probe.summary();
        match probe {
            TargetProbe::Installed => Validation {
                status: Status::Pass,
                messages: vec![summary],
            },
            TargetProbe::NotInstalled => Validation {
                status: Status::Partial,
                messages: vec![format!(
                    "{summary} (only needed for `frust build web`). Run: {}",
                    add_target_command()
                )],
            },
            TargetProbe::RustupUnavailable => Validation {
                status: Status::Partial,
                messages: vec![format!(
                    "{summary}. Install rustup (https://rustup.rs), then run: {}",
                    add_target_command()
                )],
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::FakeEnv;
    use crate::process::{FakeProcessRunner, Output};

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn ctx<'a>(runner: &'a FakeProcessRunner, env: &'a FakeEnv) -> DoctorCtx<'a> {
        DoctorCtx {
            runner,
            env,
            is_macos: false,
        }
    }

    #[test]
    fn passes_when_the_target_is_installed() {
        let runner = FakeProcessRunner::new().with(
            "rustup target list --installed",
            ok("x86_64-unknown-linux-gnu\nwasm32-unknown-unknown\n"),
        );
        let env = FakeEnv::new();
        let result = WasmTargetValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Pass);
        assert!(result.messages[0].contains(WASM_TARGET_TRIPLE));
    }

    /// Web is optional on any host: an absent target is `Partial`, never
    /// `Fail` — the doctor exit-code gate only trips on `Fail`.
    #[test]
    fn an_absent_target_is_partial_not_fail() {
        let runner = FakeProcessRunner::new().with(
            "rustup target list --installed",
            ok("x86_64-unknown-linux-gnu\n"),
        );
        let env = FakeEnv::new();
        let result = WasmTargetValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Partial);
        assert!(
            result.messages[0].contains("rustup target add wasm32-unknown-unknown"),
            "{}",
            result.messages[0]
        );
    }

    /// An unrunnable `rustup` is also `Partial`, and still names the command
    /// to run once rustup exists.
    #[test]
    fn an_unrunnable_rustup_is_partial_and_still_names_the_command() {
        let runner = FakeProcessRunner::new().missing("rustup target list --installed");
        let env = FakeEnv::new();
        let result = WasmTargetValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Partial);
        assert!(result.messages[0].contains("rustup.rs"), "{:?}", result);
        assert!(
            result.messages[0].contains("rustup target add wasm32-unknown-unknown"),
            "{:?}",
            result
        );
    }

    /// A spawned-but-failed invocation is indistinguishable from an unusable
    /// rustup for this check's purposes, and must not be read as "installed".
    #[test]
    fn a_failed_invocation_is_not_read_as_installed() {
        let runner = FakeProcessRunner::new().with(
            "rustup target list --installed",
            Output {
                success: false,
                stdout: "wasm32-unknown-unknown\n".to_string(),
                stderr: "error: no default toolchain".to_string(),
            },
        );
        assert_eq!(probe(&runner), TargetProbe::RustupUnavailable);
    }

    /// Unparseable output (no recognisable triple) is the absent case, not a
    /// panic and not a pass.
    #[test]
    fn unparseable_output_is_the_absent_case() {
        let runner =
            FakeProcessRunner::new().with("rustup target list --installed", ok("¯\\_(ツ)_/¯\n"));
        let env = FakeEnv::new();
        assert_eq!(probe(&runner), TargetProbe::NotInstalled);
        assert_eq!(
            WasmTargetValidator.validate(&ctx(&runner, &env)).status,
            Status::Partial
        );
    }
}
