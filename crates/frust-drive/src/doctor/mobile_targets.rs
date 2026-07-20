use super::{DoctorCtx, Status, Validation, Validator};
use std::collections::HashSet;

/// Required Rust compile targets for Android builds (spec §12.7) — checked
/// on every platform, since `cargo-ndk`/Gradle cross-compile Android from
/// any host.
///
/// `pub(crate)`: also read by `doctor::report`'s per-area Android/iOS
/// target breakdown (Plan D6a), sharing this list rather than duplicating it.
pub(crate) const ANDROID_TARGETS: &[&str] = &[
    "aarch64-linux-android",
    "armv7-linux-androideabi",
    "x86_64-linux-android",
];

/// Required Rust compile targets for iOS builds (task 35). Checked only on
/// macOS: `xcodebuild`/the simulator toolchain don't exist elsewhere, so
/// flagging these as missing on a non-macOS host would be a false negative
/// the user can't act on.
pub(crate) const IOS_TARGETS: &[&str] = &["aarch64-apple-ios", "aarch64-apple-ios-sim"];

/// Runs `rustup target list --installed` and returns the installed-target
/// set, or `None` if `rustup` itself couldn't be run — the shared probe
/// [`MobileTargetsValidator`] and `doctor::report`'s per-area target
/// components (Plan D6a) both build on, so the check is never duplicated.
pub(crate) fn probe_installed_targets(ctx: &DoctorCtx) -> Option<HashSet<String>> {
    match ctx.runner.run("rustup", &["target", "list", "--installed"]) {
        Ok(out) if out.success => Some(
            out.stdout
                .lines()
                .map(|line| line.trim().to_string())
                .collect(),
        ),
        _ => None,
    }
}

pub struct MobileTargetsValidator;

impl Validator for MobileTargetsValidator {
    fn name(&self) -> &str {
        "Mobile Rust targets"
    }

    fn validate(&self, ctx: &DoctorCtx) -> Validation {
        let required: Vec<&str> = if ctx.is_macos {
            ANDROID_TARGETS.iter().chain(IOS_TARGETS).copied().collect()
        } else {
            ANDROID_TARGETS.to_vec()
        };

        match probe_installed_targets(ctx) {
            Some(installed) => {
                let missing: Vec<&str> = required
                    .iter()
                    .copied()
                    .filter(|t| !installed.contains(*t))
                    .collect();
                if missing.is_empty() {
                    Validation {
                        status: Status::Pass,
                        messages: vec!["all required mobile targets installed".to_string()],
                    }
                } else {
                    Validation {
                        status: Status::Partial,
                        messages: vec![format!(
                            "missing targets. Run: rustup target add {}",
                            missing.join(" ")
                        )],
                    }
                }
            }
            None => Validation {
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
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    #[test]
    fn passes_when_all_targets_installed() {
        let runner = FakeProcessRunner::new().with(
            "rustup target list --installed",
            ok("aarch64-linux-android\narmv7-linux-androideabi\nx86_64-linux-android\naarch64-apple-ios\naarch64-apple-ios-sim\n"),
        );
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let result = MobileTargetsValidator.validate(&ctx);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn partial_when_some_targets_missing() {
        let runner = FakeProcessRunner::new().with(
            "rustup target list --installed",
            ok("aarch64-linux-android\n"),
        );
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let result = MobileTargetsValidator.validate(&ctx);
        assert_eq!(result.status, Status::Partial);
        assert!(result.messages[0].contains("rustup target add"));
        assert!(result.messages[0].contains("aarch64-apple-ios"));
    }

    #[test]
    fn passes_off_macos_without_ios_targets_installed() {
        // Android-only targets installed, no iOS targets: must still pass,
        // since iOS targets aren't required off macOS (no fixture matching
        // an "aarch64-apple-ios*"-inclusive install list is registered).
        let runner = FakeProcessRunner::new().with(
            "rustup target list --installed",
            ok("aarch64-linux-android\narmv7-linux-androideabi\nx86_64-linux-android\n"),
        );
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let result = MobileTargetsValidator.validate(&ctx);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn partial_message_omits_ios_targets_off_macos() {
        let runner = FakeProcessRunner::new().with("rustup target list --installed", ok(""));
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let result = MobileTargetsValidator.validate(&ctx);
        assert_eq!(result.status, Status::Partial);
        assert!(!result.messages[0].contains("aarch64-apple-ios"));
        assert!(result.messages[0].contains("aarch64-linux-android"));
    }

    #[test]
    fn fails_when_rustup_missing() {
        let runner = FakeProcessRunner::new().missing("rustup target list --installed");
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let result = MobileTargetsValidator.validate(&ctx);
        assert_eq!(result.status, Status::Fail);
    }
}
