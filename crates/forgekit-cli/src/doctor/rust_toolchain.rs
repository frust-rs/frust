use super::{DoctorCtx, Status, Validation, Validator};

/// Minimum supported `rustc` version (workspace `rust-version`, spec §4).
const MIN_RUST_VERSION: (u64, u64, u64) = (1, 88, 0);

pub struct RustToolchainValidator;

impl Validator for RustToolchainValidator {
    fn name(&self) -> &str {
        "Rust toolchain"
    }

    fn validate(&self, ctx: &DoctorCtx) -> Validation {
        let mut messages = Vec::new();
        let mut status = Status::Pass;

        match ctx.runner.run("rustc", &["--version"]) {
            Ok(out) if out.success => match parse_version(&out.stdout) {
                Some(version) if version >= MIN_RUST_VERSION => {
                    messages.push(format!("rustc {}.{}.{}", version.0, version.1, version.2));
                }
                Some(version) => {
                    status = Status::Fail;
                    messages.push(format!(
                        "rustc {}.{}.{} found, but ForgeKit requires >= {}.{}.{}. Run `rustup update`.",
                        version.0, version.1, version.2, MIN_RUST_VERSION.0, MIN_RUST_VERSION.1, MIN_RUST_VERSION.2
                    ));
                }
                None => {
                    status = Status::Partial;
                    messages.push(format!(
                        "could not parse rustc version from: {}",
                        out.stdout.trim()
                    ));
                }
            },
            _ => {
                status = Status::Fail;
                messages.push("rustc not found. Install via https://rustup.rs".to_string());
            }
        }

        match ctx.runner.run("cargo", &["--version"]) {
            Ok(out) if out.success => messages.push("cargo present".to_string()),
            _ => {
                status = Status::Fail;
                messages.push("cargo not found. Install via https://rustup.rs".to_string());
            }
        }

        Validation { status, messages }
    }
}

/// Parses `"rustc 1.91.1 (ed61e7d7e 2025-11-07)"` into `(1, 91, 1)`.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let version = text.split_whitespace().nth(1)?;
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
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
    fn passes_when_version_meets_minimum() {
        let runner = FakeProcessRunner::new()
            .with(
                "rustc --version",
                ok("rustc 1.91.1 (ed61e7d7e 2025-11-07)\n"),
            )
            .with("cargo --version", ok("cargo 1.91.1\n"));
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let result = RustToolchainValidator.validate(&ctx);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn fails_when_version_below_minimum() {
        let runner = FakeProcessRunner::new()
            .with("rustc --version", ok("rustc 1.70.0 (abc 2023-01-01)\n"))
            .with("cargo --version", ok("cargo 1.70.0\n"));
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let result = RustToolchainValidator.validate(&ctx);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn fails_when_rustc_missing() {
        let runner = FakeProcessRunner::new()
            .missing("rustc --version")
            .with("cargo --version", ok("cargo 1.91.1"));
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let result = RustToolchainValidator.validate(&ctx);
        assert_eq!(result.status, Status::Fail);
        assert!(result.messages.iter().any(|m| m.contains("rustup.rs")));
    }

    #[test]
    fn parses_version_string() {
        assert_eq!(
            parse_version("rustc 1.88.0 (deadbeef 2025-01-01)"),
            Some((1, 88, 0))
        );
        assert_eq!(parse_version("garbage"), None);
    }
}
