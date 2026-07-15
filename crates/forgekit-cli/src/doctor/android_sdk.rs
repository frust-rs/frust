use super::{DoctorCtx, Status, Validation, Validator};

pub struct AndroidSdkValidator;

impl Validator for AndroidSdkValidator {
    fn name(&self) -> &str {
        "Android toolchain"
    }

    fn validate(&self, ctx: &DoctorCtx) -> Validation {
        let mut messages = Vec::new();
        let mut status = Status::Pass;

        let sdk_home = ctx
            .env
            .get("ANDROID_HOME")
            .or_else(|| ctx.env.get("ANDROID_SDK_ROOT"));
        let adb_ok = matches!(ctx.runner.run("adb", &["version"]), Ok(out) if out.success);

        match (&sdk_home, adb_ok) {
            (Some(home), true) => messages.push(format!("Android SDK at {home}; adb on PATH")),
            (Some(home), false) => {
                status = Status::Partial;
                messages.push(format!(
                    "Android SDK at {home}, but `adb` was not found. Add {home}/platform-tools to PATH."
                ));
            }
            (None, true) => {
                status = Status::Partial;
                messages.push(
                    "`adb` found on PATH, but ANDROID_HOME/ANDROID_SDK_ROOT is not set."
                        .to_string(),
                );
            }
            (None, false) => {
                status = Status::Fail;
                messages.push(
                    "Android SDK not found. Install Android Studio / the SDK and set ANDROID_HOME."
                        .to_string(),
                );
            }
        }

        match ctx.env.get("ANDROID_NDK_HOME") {
            Some(ndk) => messages.push(format!("Android NDK at {ndk}")),
            None => {
                if status == Status::Pass {
                    status = Status::Partial;
                }
                messages.push(
                    "ANDROID_NDK_HOME not set. Install NDK r26 via `sdkmanager --install \"ndk;26.1.10909125\"` \
                     and set ANDROID_NDK_HOME."
                        .to_string(),
                );
            }
        }

        Validation { status, messages }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::FakeEnv;
    use crate::process::{FakeProcessRunner, Output};

    fn ok() -> Output {
        Output {
            success: true,
            stdout: "1.0.41\n".to_string(),
            stderr: String::new(),
        }
    }

    #[test]
    fn passes_when_sdk_and_ndk_and_adb_present() {
        let runner = FakeProcessRunner::new().with("adb version", ok());
        let env = FakeEnv::new()
            .set("ANDROID_HOME", "/sdk")
            .set("ANDROID_NDK_HOME", "/sdk/ndk/26.1.10909125");
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let result = AndroidSdkValidator.validate(&ctx);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn partial_when_ndk_home_missing() {
        let runner = FakeProcessRunner::new().with("adb version", ok());
        let env = FakeEnv::new().set("ANDROID_HOME", "/sdk");
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let result = AndroidSdkValidator.validate(&ctx);
        assert_eq!(result.status, Status::Partial);
        assert!(
            result
                .messages
                .iter()
                .any(|m| m.contains("ANDROID_NDK_HOME"))
        );
    }

    #[test]
    fn fails_when_nothing_present() {
        let runner = FakeProcessRunner::new().missing("adb version");
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let result = AndroidSdkValidator.validate(&ctx);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn partial_when_sdk_env_set_but_adb_missing() {
        let runner = FakeProcessRunner::new().missing("adb version");
        let env = FakeEnv::new()
            .set("ANDROID_SDK_ROOT", "/sdk")
            .set("ANDROID_NDK_HOME", "/sdk/ndk");
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let result = AndroidSdkValidator.validate(&ctx);
        assert_eq!(result.status, Status::Partial);
    }
}
