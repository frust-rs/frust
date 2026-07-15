use super::{DoctorCtx, Status, Validation, Validator};

pub struct CargoNdkValidator;

impl Validator for CargoNdkValidator {
    fn name(&self) -> &str {
        "cargo-ndk"
    }

    fn validate(&self, ctx: &DoctorCtx) -> Validation {
        match ctx.runner.run("cargo", &["ndk", "--version"]) {
            Ok(out) if out.success => {
                let version = out.stdout.lines().next().unwrap_or("cargo-ndk").trim().to_string();
                Validation { status: Status::Pass, messages: vec![version] }
            }
            _ => Validation {
                status: Status::Fail,
                messages: vec!["cargo-ndk not found. Run: cargo install cargo-ndk".to_string()],
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::FakeEnv;
    use crate::process::{FakeProcessRunner, Output};

    #[test]
    fn passes_when_cargo_ndk_present() {
        let runner = FakeProcessRunner::new().with(
            "cargo ndk --version",
            Output { success: true, stdout: "cargo-ndk 3.5.4\n".to_string(), stderr: String::new() },
        );
        let env = FakeEnv::new();
        let ctx = DoctorCtx { runner: &runner, env: &env, is_macos: true };
        let result = CargoNdkValidator.validate(&ctx);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn fails_when_cargo_ndk_missing() {
        let runner = FakeProcessRunner::new().missing("cargo ndk --version");
        let env = FakeEnv::new();
        let ctx = DoctorCtx { runner: &runner, env: &env, is_macos: true };
        let result = CargoNdkValidator.validate(&ctx);
        assert_eq!(result.status, Status::Fail);
        assert!(result.messages[0].contains("cargo install cargo-ndk"));
    }
}
