//! `wasm-opt` (binaryen) presence check for the browser pipeline
//! (`crate::web_build`).
//!
//! **The weakest claim any doctor row makes.** `wasm-opt` is a size
//! optimization applied strictly after `wasm-bindgen`; a browser build
//! without it produces a byte-for-byte functional artifact, just a larger
//! one. So its absence is a [`Status::Partial`] here and a
//! `ComponentStatus::Partial` in `crate::web_build::preflight` — the one
//! browser check that cannot block a build on either surface — and it never
//! trips `commands::doctor::run_in`'s exit-1 gate.

use super::{DoctorCtx, Status, Validation, Validator};
use crate::process::ProcessRunner;

/// What `wasm-opt --version` says about the installed optimizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WasmOptProbe {
    /// The tool's own first version line, verbatim (`wasm-opt version 130`).
    Present(String),
    Absent,
}

/// What both surfaces say when the tool is absent: the same sentence in the
/// flat doctor row's message and in `web_build::preflight`'s component
/// summary, so neither can understate or overstate the consequence.
pub(crate) const ABSENT_SUMMARY: &str = "not on PATH — builds succeed without it, the shipped \
                                         .wasm is just larger; install binaryen";

/// The guidance text `doctor::report` and `web_build::preflight` both attach
/// to an absent `wasm-opt` row: there is no one install command (binaryen
/// ships as a platform package — brew, apt, a release archive), so this is
/// guidance with a doc link, never a runnable [`super::FixCommand`].
pub(crate) const INSTALL_GUIDANCE: &str = "Install binaryen (provides wasm-opt)";

/// The doc link paired with [`INSTALL_GUIDANCE`], kept beside it so the two
/// surfaces that display it can never drift apart.
pub(crate) const INSTALL_DOC_LINK: &str = "https://github.com/WebAssembly/binaryen/releases";

/// The single `wasm-opt` probe: one invocation, one classification, shared by
/// [`WasmOptValidator`] and `web_build::preflight`'s wasm-opt row.
pub(crate) fn probe(runner: &dyn ProcessRunner) -> WasmOptProbe {
    match runner.run("wasm-opt", &["--version"]) {
        Ok(out) if out.success => WasmOptProbe::Present(
            out.stdout
                .lines()
                .next()
                .unwrap_or("installed")
                .trim()
                .to_string(),
        ),
        _ => WasmOptProbe::Absent,
    }
}

pub struct WasmOptValidator;

impl Validator for WasmOptValidator {
    fn name(&self) -> &str {
        "wasm-opt"
    }

    fn validate(&self, ctx: &DoctorCtx) -> Validation {
        match probe(ctx.runner) {
            WasmOptProbe::Present(version_line) => Validation {
                status: Status::Pass,
                messages: vec![version_line],
            },
            WasmOptProbe::Absent => Validation {
                status: Status::Partial,
                messages: vec![ABSENT_SUMMARY.to_string()],
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
    fn passes_with_the_tools_own_version_line() {
        let runner = FakeProcessRunner::new().with(
            "wasm-opt --version",
            ok("wasm-opt version 130 (version_130)\nextra\n"),
        );
        let env = FakeEnv::new();
        let result = WasmOptValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.messages[0], "wasm-opt version 130 (version_130)");
    }

    /// The optional tool's whole contract: absent is worse, never fatal.
    #[test]
    fn absence_is_partial_not_fail() {
        let runner = FakeProcessRunner::new().missing("wasm-opt --version");
        let env = FakeEnv::new();
        let result = WasmOptValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Partial);
        assert_eq!(result.messages[0], ABSENT_SUMMARY);
        assert!(result.messages[0].contains("binaryen"));
    }

    #[test]
    fn a_failed_but_spawned_invocation_is_the_absent_case() {
        let runner = FakeProcessRunner::new().with(
            "wasm-opt --version",
            Output {
                success: false,
                stdout: "wasm-opt version 130\n".to_string(),
                stderr: "error".to_string(),
            },
        );
        let env = FakeEnv::new();
        assert_eq!(probe(&runner), WasmOptProbe::Absent);
        assert_eq!(
            WasmOptValidator.validate(&ctx(&runner, &env)).status,
            Status::Partial
        );
    }

    /// Empty output from a successful invocation still counts as present —
    /// the tool answered, there is just nothing to quote.
    #[test]
    fn empty_output_from_a_successful_run_is_still_present() {
        let runner = FakeProcessRunner::new().with("wasm-opt --version", ok(""));
        assert_eq!(
            probe(&runner),
            WasmOptProbe::Present("installed".to_string())
        );
    }
}
