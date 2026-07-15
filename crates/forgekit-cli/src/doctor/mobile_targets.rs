use super::{DoctorCtx, Status, Validation, Validator};
use std::collections::HashSet;

/// Required Rust compile targets for mobile builds (spec §12.7).
const REQUIRED_TARGETS: &[&str] = &[
    "aarch64-linux-android",
    "armv7-linux-androideabi",
    "x86_64-linux-android",
    "aarch64-apple-ios",
    "aarch64-apple-ios-sim",
];

pub struct MobileTargetsValidator;

impl Validator for MobileTargetsValidator {
    fn name(&self) -> &str {
        "Mobile Rust targets"
    }

    fn validate(&self, ctx: &DoctorCtx) -> Validation {
        match ctx.runner.run("rustup", &["target", "list", "--installed"]) {
            Ok(out) if out.success => {
                let installed: HashSet<&str> = out.stdout.lines().map(str::trim).collect();
                let missing: Vec<&str> =
                    REQUIRED_TARGETS.iter().copied().filter(|t| !installed.contains(t)).collect();
                if missing.is_empty() {
                    Validation {
                        status: Status::Pass,
                        messages: vec!["all required mobile targets installed".to_string()],
                    }
                } else {
                    Validation {
                        status: Status::Partial,
                        messages: vec![format!("missing targets. Run: rustup target add {}", missing.join(" "))],
                    }
                }
            }
            _ => Validation {
                status: Status::Fail,
                messages: vec!["rustup not found. Install via https://rustup.rs".to_string()],
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
        Output { success: true, stdout: stdout.to_string(), stderr: String::new() }
    }

    #[test]
    fn passes_when_all_targets_installed() {
        let runner = FakeProcessRunner::new().with(
            "rustup target list --installed",
            ok("aarch64-linux-android\narmv7-linux-androideabi\nx86_64-linux-android\naarch64-apple-ios\naarch64-apple-ios-sim\n"),
        );
        let env = FakeEnv::new();
        let ctx = DoctorCtx { runner: &runner, env: &env, is_macos: true };
        let result = MobileTargetsValidator.validate(&ctx);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn partial_when_some_targets_missing() {
        let runner = FakeProcessRunner::new()
            .with("rustup target list --installed", ok("aarch64-linux-android\n"));
        let env = FakeEnv::new();
        let ctx = DoctorCtx { runner: &runner, env: &env, is_macos: true };
        let result = MobileTargetsValidator.validate(&ctx);
        assert_eq!(result.status, Status::Partial);
        assert!(result.messages[0].contains("rustup target add"));
        assert!(result.messages[0].contains("aarch64-apple-ios"));
    }

    #[test]
    fn fails_when_rustup_missing() {
        let runner = FakeProcessRunner::new().missing("rustup target list --installed");
        let env = FakeEnv::new();
        let ctx = DoctorCtx { runner: &runner, env: &env, is_macos: true };
        let result = MobileTargetsValidator.validate(&ctx);
        assert_eq!(result.status, Status::Fail);
    }
}
