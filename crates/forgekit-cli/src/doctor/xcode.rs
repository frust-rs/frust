use super::{DoctorCtx, Status, Validation, Validator};

/// Minimum supported Xcode major version (accesskit_ios / xcodebuild floor, spec §12.7).
const MIN_XCODE_MAJOR: u32 = 15;

pub struct XcodeValidator;

impl Validator for XcodeValidator {
    fn name(&self) -> &str {
        "Xcode"
    }

    fn validate(&self, ctx: &DoctorCtx) -> Validation {
        if !ctx.is_macos {
            return Validation {
                status: Status::Pass,
                messages: vec!["skipped (not macOS)".to_string()],
            };
        }

        let select_ok = matches!(ctx.runner.run("xcode-select", &["-p"]), Ok(out) if out.success);
        if !select_ok {
            return Validation {
                status: Status::Fail,
                messages: vec!["Xcode not found. Run: xcode-select --install".to_string()],
            };
        }

        match ctx.runner.run("xcodebuild", &["-version"]) {
            Ok(out) if out.success => match parse_major_version(&out.stdout) {
                Some(major) if major >= MIN_XCODE_MAJOR => Validation {
                    status: Status::Pass,
                    messages: vec![out.stdout.lines().next().unwrap_or_default().to_string()],
                },
                Some(major) => Validation {
                    status: Status::Fail,
                    messages: vec![format!(
                        "Xcode {major} found, but ForgeKit requires Xcode {MIN_XCODE_MAJOR}+."
                    )],
                },
                None => Validation {
                    status: Status::Partial,
                    messages: vec![format!(
                        "could not parse Xcode version from: {}",
                        out.stdout.trim()
                    )],
                },
            },
            _ => Validation {
                status: Status::Fail,
                messages: vec!["xcodebuild not found or failed to run.".to_string()],
            },
        }
    }
}

/// Parses `"Xcode 15.4\nBuild version 15F31d"` into `15`.
fn parse_major_version(text: &str) -> Option<u32> {
    let first_line = text.lines().next()?;
    let version = first_line.split_whitespace().nth(1)?;
    version.split('.').next()?.parse().ok()
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

    #[test]
    fn skips_cleanly_on_non_macos() {
        let runner = FakeProcessRunner::new();
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let result = XcodeValidator.validate(&ctx);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn passes_when_xcode_15_plus() {
        let runner = FakeProcessRunner::new()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "xcodebuild -version",
                ok("Xcode 15.4\nBuild version 15F31d\n"),
            );
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let result = XcodeValidator.validate(&ctx);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn fails_when_xcode_too_old() {
        let runner = FakeProcessRunner::new()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "xcodebuild -version",
                ok("Xcode 14.3\nBuild version 14E222b\n"),
            );
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let result = XcodeValidator.validate(&ctx);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn fails_when_xcode_select_fails() {
        let runner = FakeProcessRunner::new().missing("xcode-select -p");
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let result = XcodeValidator.validate(&ctx);
        assert_eq!(result.status, Status::Fail);
    }
}
