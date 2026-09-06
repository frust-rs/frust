//! `cargo-packager` presence/version check for the desktop-installer
//! pipeline (`crate::desktop_build::installer`).
//!
//! **Non-fatal by design.** Every other validator in [`super::default_validators`]
//! can legitimately fail `frust doctor`'s exit code — `rustc`, the Android
//! SDK/NDK, Xcode are all needed for the pipelines they gate. `cargo-packager`
//! isn't: a plain `frust build macos|windows|linux` bundle never shells out to
//! it, only `frust build --installer <format>` does, so its absence is a
//! [`Status::Partial`] at worst, never [`Status::Fail`] — this validator
//! never trips `commands::doctor::run_in`'s exit-1 gate.

use super::{DoctorCtx, Status, Validation, Validator};
use crate::desktop_build::CARGO_PACKAGER_PINNED;

pub struct CargoPackagerValidator;

impl Validator for CargoPackagerValidator {
    fn name(&self) -> &str {
        "cargo-packager"
    }

    fn validate(&self, ctx: &DoctorCtx) -> Validation {
        match ctx.runner.run("cargo", &["packager", "--version"]) {
            Ok(out) if out.success => match parse_version(&out.stdout) {
                Some(version) if version == CARGO_PACKAGER_PINNED => Validation {
                    status: Status::Pass,
                    messages: vec![format!("cargo-packager {version}")],
                },
                Some(version) => Validation {
                    status: Status::Partial,
                    messages: vec![format!(
                        "cargo-packager {version} found, but Frust pins {CARGO_PACKAGER_PINNED} \
                         (only needed for `frust build --installer`). Run: cargo install \
                         cargo-packager --version {CARGO_PACKAGER_PINNED} --locked"
                    )],
                },
                None => Validation {
                    status: Status::Partial,
                    messages: vec![format!(
                        "could not parse cargo-packager's version from: {}",
                        out.stdout.trim()
                    )],
                },
            },
            _ => Validation {
                status: Status::Partial,
                messages: vec![format!(
                    "cargo-packager not found (only needed for `frust build --installer`). \
                     Run: cargo install cargo-packager --version {CARGO_PACKAGER_PINNED} --locked"
                )],
            },
        }
    }
}

/// Parses `"cargo-packager 0.11.8\n"` into `"0.11.8"`.
fn parse_version(text: &str) -> Option<String> {
    let first_line = text.lines().next()?;
    first_line.split_whitespace().nth(1).map(str::to_string)
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
            "cargo packager --version",
            ok(&format!("cargo-packager {CARGO_PACKAGER_PINNED}\n")),
        );
        let env = FakeEnv::new();
        let result = CargoPackagerValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Pass);
        assert!(result.messages[0].contains(CARGO_PACKAGER_PINNED));
    }

    /// Absence is non-fatal: `Partial`, never `Fail` — the doctor exit-code
    /// gate only trips on `Fail`.
    #[test]
    fn absence_is_partial_not_fail() {
        let runner = FakeProcessRunner::new().missing("cargo packager --version");
        let env = FakeEnv::new();
        let result = CargoPackagerValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Partial);
        assert!(result.messages[0].contains("cargo install cargo-packager"));
        assert!(result.messages[0].contains(CARGO_PACKAGER_PINNED));
    }

    /// A pinned-tool mismatch is also non-fatal — `Partial`, naming the exact
    /// fix command, same as the missing case.
    #[test]
    fn a_version_mismatch_is_partial_not_fail() {
        let runner =
            FakeProcessRunner::new().with("cargo packager --version", ok("cargo-packager 0.9.0\n"));
        let env = FakeEnv::new();
        let result = CargoPackagerValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Partial);
        assert!(result.messages[0].contains("0.9.0"));
        assert!(result.messages[0].contains(CARGO_PACKAGER_PINNED));
    }

    #[test]
    fn an_unparseable_version_line_is_partial_not_a_panic() {
        let runner =
            FakeProcessRunner::new().with("cargo packager --version", ok("not-a-version-line\n"));
        let env = FakeEnv::new();
        let result = CargoPackagerValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Partial);
    }

    #[test]
    fn a_failed_but_spawned_invocation_is_partial_not_fail() {
        let runner = FakeProcessRunner::new().with(
            "cargo packager --version",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error: no such subcommand".to_string(),
            },
        );
        let env = FakeEnv::new();
        let result = CargoPackagerValidator.validate(&ctx(&runner, &env));
        assert_eq!(result.status, Status::Partial);
    }
}
