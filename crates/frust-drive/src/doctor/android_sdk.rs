use std::path::Path;

use super::{DoctorCtx, Status, Validation, Validator};

/// The NDK package the generated Android project is built and tested with.
const NDK_PACKAGE: &str = "ndk;28.2.13676358";

/// First NDK major version whose shared libraries are 16 KB aligned by default.
const MIN_ALIGNED_NDK_MAJOR: u32 = 28;

/// Parses the `Pkg.Revision` value out of an NDK `source.properties` body and
/// returns its major version, or `None` when the key or a numeric major is absent.
fn parse_ndk_major(source_properties: &str) -> Option<u32> {
    source_properties.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim() != "Pkg.Revision" {
            return None;
        }
        value.trim().split('.').next()?.parse().ok()
    })
}

/// Reads the NDK major version from `<ndk_dir>/source.properties`.
fn read_ndk_major(ndk_dir: &Path) -> Option<u32> {
    let body = std::fs::read_to_string(ndk_dir.join("source.properties")).ok()?;
    parse_ndk_major(&body)
}

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
            Some(ndk) => match read_ndk_major(Path::new(&ndk)) {
                Some(major) if major >= MIN_ALIGNED_NDK_MAJOR => {
                    messages.push(format!("Android NDK r{major} at {ndk}"));
                }
                Some(major) => {
                    if status == Status::Pass {
                        status = Status::Partial;
                    }
                    messages.push(format!(
                        "Android NDK r{major} at {ndk} is older than r{MIN_ALIGNED_NDK_MAJOR}: \
                         native libraries will not be 16 KB aligned. Install it via \
                         `sdkmanager --install \"{NDK_PACKAGE}\"` and point ANDROID_NDK_HOME at it."
                    ));
                }
                None => messages.push(format!("Android NDK at {ndk} (version not verified)")),
            },
            None => {
                if status == Status::Pass {
                    status = Status::Partial;
                }
                messages.push(format!(
                    "ANDROID_NDK_HOME not set. Install NDK r{MIN_ALIGNED_NDK_MAJOR} via \
                         `sdkmanager --install \"{NDK_PACKAGE}\"` and set ANDROID_NDK_HOME."
                ));
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
            .set("ANDROID_NDK_HOME", "/sdk/ndk/28.2.13676358");
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

    fn ndk_dir(name: &str, revision: Option<&str>) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("frust-ndk-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(rev) = revision {
            std::fs::write(
                dir.join("source.properties"),
                format!("Pkg.Desc = Android NDK\nPkg.Revision = {rev}\n"),
            )
            .unwrap();
        }
        dir
    }

    fn validate_with_ndk(dir: &std::path::Path) -> Validation {
        let runner = FakeProcessRunner::new().with("adb version", ok());
        let env = FakeEnv::new()
            .set("ANDROID_HOME", "/sdk")
            .set("ANDROID_NDK_HOME", dir.to_str().unwrap());
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        AndroidSdkValidator.validate(&ctx)
    }

    #[test]
    fn parses_well_formed_revision() {
        assert_eq!(
            parse_ndk_major("Pkg.Desc = Android NDK\nPkg.Revision = 28.2.13676358\n"),
            Some(28)
        );
    }

    #[test]
    fn missing_source_properties_yields_none() {
        let dir = ndk_dir("missing", None);
        assert_eq!(read_ndk_major(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_revision_yields_none() {
        assert_eq!(parse_ndk_major("Pkg.Revision = banana\n"), None);
        assert_eq!(parse_ndk_major("nothing here"), None);
        assert_eq!(parse_ndk_major(""), None);
    }

    #[test]
    fn r27_warns_and_names_package() {
        let dir = ndk_dir("r27", Some("27.1.12297006"));
        let result = validate_with_ndk(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(result.status, Status::Partial);
        assert!(
            result
                .messages
                .iter()
                .any(|m| { m.contains("16 KB") && m.contains("ndk;28.2.13676358") })
        );
    }

    #[test]
    fn r28_passes() {
        let dir = ndk_dir("r28", Some("28.2.13676358"));
        let result = validate_with_ndk(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn unset_ndk_home_message_names_r28() {
        let runner = FakeProcessRunner::new().with("adb version", ok());
        let env = FakeEnv::new().set("ANDROID_HOME", "/sdk");
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let result = AndroidSdkValidator.validate(&ctx);
        assert!(result.messages.iter().any(|m| {
            m.contains("r28") && m.contains("ndk;28.2.13676358") && !m.contains("r26")
        }));
    }
}
