//! Host `wasm-bindgen` CLI presence/version check for the browser pipeline
//! (`crate::web_build`).
//!
//! **Non-fatal by design**, like [`super::CargoPackagerValidator`]: nothing
//! but `frust build web` shells out to this CLI, so its absence or a version
//! disagreement is a [`Status::Partial`] at worst and never trips
//! `commands::doctor::run_in`'s exit-1 gate.
//!
//! # Two pins, and which one this row checks
//!
//! The CLI's version and the `wasm-bindgen` *crate*'s version share a schema
//! and must be equal — an exact-equality contract, not a semver range (see
//! `crate::web_build::preflight`'s module doc). This row compares against
//! [`WASM_BINDGEN_PINNED`], the version **the framework** pins, because a
//! doctor row answers "is this host set up for Frust" with no project in
//! hand. A project declaring its own `wasm-bindgen` dependency is checked
//! against *that* instead, by `web_build::preflight`'s row, and the project's
//! pin wins where the two differ — the message below says so rather than
//! leaving a developer to choose between two numbers.

use super::{DoctorCtx, Status, Validation, Validator};
use crate::process::ProcessRunner;
use crate::web_build::WASM_BINDGEN_PINNED;

/// What `wasm-bindgen --version` says about the installed CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BindgenProbe {
    Version(String),
    /// The CLI ran but printed something this parser does not recognise —
    /// reported honestly rather than guessed at.
    Unparseable(String),
    Absent,
}

/// The single installed-CLI probe: one invocation, one banner parse, shared
/// by [`WasmBindgenCliValidator`] and `web_build::preflight`'s bindgen row.
pub(crate) fn probe(runner: &dyn ProcessRunner) -> BindgenProbe {
    let Ok(out) = runner.run("wasm-bindgen", &["--version"]) else {
        return BindgenProbe::Absent;
    };
    if !out.success {
        return BindgenProbe::Absent;
    }
    match parse_version(&out.stdout) {
        Some(version) => BindgenProbe::Version(version),
        None => BindgenProbe::Unparseable(out.stdout.trim().to_string()),
    }
}

/// The installed CLI's version, or `None` when it is absent or unparseable —
/// the shape `web_build::preflight`'s project-pin comparison wants.
pub(crate) fn installed_version(runner: &dyn ProcessRunner) -> Option<String> {
    match probe(runner) {
        BindgenProbe::Version(version) => Some(version),
        BindgenProbe::Unparseable(_) | BindgenProbe::Absent => None,
    }
}

/// Parses `"wasm-bindgen 0.2.128\n"` into `"0.2.128"`.
fn parse_version(text: &str) -> Option<String> {
    let first_line = text.lines().next()?;
    first_line.split_whitespace().nth(1).map(str::to_string)
}

/// The install command for a given version, as a human string. The structured
/// form of the same fix is `doctor::report`'s/`web_build::preflight`'s
/// `FixCommand`.
pub(crate) fn install_command(version: &str) -> String {
    format!("cargo install -f wasm-bindgen-cli --version {version}")
}

/// Why a project's own pin is the one that wins — appended to every message
/// that names [`WASM_BINDGEN_PINNED`], so the framework pin is never read as
/// an instruction to downgrade a project that pins something else.
const PROJECT_PIN_NOTE: &str =
    "a project pinning a different wasm-bindgen wins — install the version that project declares";

pub struct WasmBindgenCliValidator;

impl Validator for WasmBindgenCliValidator {
    fn name(&self) -> &str {
        "wasm-bindgen CLI"
    }

    fn validate(&self, ctx: &DoctorCtx) -> Validation {
        match probe(ctx.runner) {
            BindgenProbe::Version(version) if version == WASM_BINDGEN_PINNED => Validation {
                status: Status::Pass,
                messages: vec![format!("wasm-bindgen {version}")],
            },
            BindgenProbe::Version(version) => Validation {
                status: Status::Partial,
                messages: vec![format!(
                    "wasm-bindgen {version} found, but Frust pins {WASM_BINDGEN_PINNED} — the CLI \
                     and the crate share a schema version and must be equal. Run: {}. Note that \
                     {PROJECT_PIN_NOTE}.",
                    install_command(WASM_BINDGEN_PINNED)
                )],
            },
            BindgenProbe::Unparseable(banner) => Validation {
                status: Status::Partial,
                messages: vec![format!(
                    "could not parse wasm-bindgen's version from: {banner}"
                )],
            },
            BindgenProbe::Absent => Validation {
                status: Status::Partial,
                messages: vec![format!(
                    "wasm-bindgen not found (only needed for `frust build web`). Run: {}. Note \
                     that {PROJECT_PIN_NOTE}.",
                    install_command(WASM_BINDGEN_PINNED)
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
    fn passes_when_present_and_pinned() {
        let runner = FakeProcessRunner::new().with(
            "wasm-bindgen --version",
            ok(&format!("wasm-bindgen {WASM_BINDGEN_PINNED}\n")),
        );
        let env = FakeEnv::new();
        let result = WasmBindgenCliValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Pass);
        assert!(result.messages[0].contains(WASM_BINDGEN_PINNED));
    }

    /// A version disagreement is non-fatal — `Partial`, naming the install
    /// command and stating that a project's own pin outranks the framework's.
    #[test]
    fn a_version_mismatch_is_partial_and_defers_to_a_project_pin() {
        let runner =
            FakeProcessRunner::new().with("wasm-bindgen --version", ok("wasm-bindgen 0.2.100\n"));
        let env = FakeEnv::new();
        let result = WasmBindgenCliValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Partial);
        assert!(result.messages[0].contains("0.2.100"), "{:?}", result);
        assert!(
            result.messages[0].contains(&install_command(WASM_BINDGEN_PINNED)),
            "{:?}",
            result
        );
        assert!(result.messages[0].contains(PROJECT_PIN_NOTE), "{result:?}");
    }

    #[test]
    fn absence_is_partial_not_fail() {
        let runner = FakeProcessRunner::new().missing("wasm-bindgen --version");
        let env = FakeEnv::new();
        let result = WasmBindgenCliValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Partial);
        assert!(
            result.messages[0].contains(&install_command(WASM_BINDGEN_PINNED)),
            "{:?}",
            result
        );
        assert_eq!(installed_version(&runner), None);
    }

    #[test]
    fn an_unparseable_version_line_is_partial_not_a_panic() {
        let runner =
            FakeProcessRunner::new().with("wasm-bindgen --version", ok("not-a-version-line\n"));
        let env = FakeEnv::new();
        let result = WasmBindgenCliValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Partial);
        assert_eq!(
            probe(&runner),
            BindgenProbe::Unparseable("not-a-version-line".to_string())
        );
        assert_eq!(installed_version(&runner), None);
    }

    #[test]
    fn a_failed_but_spawned_invocation_is_partial_not_fail() {
        let runner = FakeProcessRunner::new().with(
            "wasm-bindgen --version",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error: unknown flag".to_string(),
            },
        );
        let env = FakeEnv::new();
        assert_eq!(
            WasmBindgenCliValidator.validate(&ctx(&runner, &env)).status,
            Status::Partial
        );
        assert_eq!(probe(&runner), BindgenProbe::Absent);
    }
}
