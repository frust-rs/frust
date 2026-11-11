use std::path::{Path, PathBuf};

use super::{DoctorCtx, Status, Validation, Validator};

/// The NDK version the generated Android project builds with unless
/// `FRUST_ANDROID_NDK_VERSION` names another. The generated `build.gradle.kts`
/// carries its own copy of this literal (it is a template file); a test below
/// reads the template and fails if the two drift.
pub(crate) const DEFAULT_NDK_VERSION: &str = "28.2.13676358";

/// The `sdkmanager` package name for an NDK version, e.g. `ndk;28.2.13676358`.
pub(crate) fn ndk_package(version: &str) -> String {
    format!("ndk;{version}")
}

/// The `sdkmanager` package for [`DEFAULT_NDK_VERSION`].
pub(crate) fn default_ndk_package() -> String {
    ndk_package(DEFAULT_NDK_VERSION)
}

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

/// The NDK version the generated build asks for: `FRUST_ANDROID_NDK_VERSION`
/// when set and non-blank, else [`DEFAULT_NDK_VERSION`].
fn wanted_ndk_version(ctx: &DoctorCtx) -> String {
    ctx.env
        .get("FRUST_ANDROID_NDK_VERSION")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_NDK_VERSION.to_string())
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

        // The generated build resolves `<sdk>/ndk/<version>`, where `<sdk>` is `sdk.dir` from the
        // project's android/local.properties, else ANDROID_HOME, else ANDROID_SDK_ROOT. Doctor has
        // no project directory, so it cannot read local.properties and checks the two environment
        // variables only: a project that sets `sdk.dir` elsewhere can differ from this verdict.
        let Some(sdk) = sdk_home.as_deref() else {
            // The SDK-missing message above already says what to fix.
            return Validation { status, messages };
        };
        let version = wanted_ndk_version(ctx);
        let package = ndk_package(&version);
        let ndk_dir: PathBuf = Path::new(sdk).join("ndk").join(&version);
        let shown = ndk_dir.display();
        let warn = |status: &mut Status| {
            if *status == Status::Pass {
                *status = Status::Partial;
            }
        };

        if !ndk_dir.is_dir() {
            warn(&mut status);
            messages.push(format!(
                "Android NDK {version} not found at {shown}. Install it with \
                 `sdkmanager --install \"{package}\"`, or set FRUST_ANDROID_NDK_VERSION to an \
                 installed r{MIN_ALIGNED_NDK_MAJOR}+ version."
            ));
        } else {
            match read_ndk_major(&ndk_dir) {
                Some(major) if major >= MIN_ALIGNED_NDK_MAJOR => {
                    messages.push(format!("Android NDK r{major} at {shown}"));
                }
                Some(major) => {
                    warn(&mut status);
                    messages.push(format!(
                        "Android NDK r{major} at {shown} is older than r{MIN_ALIGNED_NDK_MAJOR}: \
                         native libraries will not be 16 KB aligned. Install \
                         `sdkmanager --install \"{}\"` and unset FRUST_ANDROID_NDK_VERSION, or set \
                         it to an installed r{MIN_ALIGNED_NDK_MAJOR}+ version.",
                        default_ndk_package()
                    ));
                }
                None => {
                    warn(&mut status);
                    messages.push(format!(
                        "Android NDK at {shown} (version not verified: no readable Pkg.Revision in \
                         its source.properties). Reinstall it with \
                         `sdkmanager --install \"{package}\"`."
                    ));
                }
            }
        }

        // Informational only: the verdict above never depends on ANDROID_NDK_HOME.
        if let Some(home) = ctx.env.get("ANDROID_NDK_HOME")
            && !home.trim().is_empty()
            && Path::new(home.trim()) != ndk_dir
        {
            messages.push(format!(
                "ANDROID_NDK_HOME is {home}, which is not the build's NDK ({shown}): `frust run` / \
                 `frust build` use the SDK copy and ignore ANDROID_NDK_HOME; the hot-patch path \
                 uses ANDROID_NDK_HOME."
            ));
        }

        Validation { status, messages }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A temporary Android SDK root that is removed when dropped, so a failed
    /// assertion cannot leak it. `DoctorCtx` offers no seam for faking file
    /// reads, so the validator tests build a real directory tree instead.
    pub(crate) struct TempSdk(PathBuf);

    impl TempSdk {
        pub(crate) fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("frust-sdk-test-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// An SDK with the default NDK installed at r28.
        pub(crate) fn with_default_ndk() -> Self {
            let sdk = Self::new();
            sdk.install_ndk(super::DEFAULT_NDK_VERSION, Some(super::DEFAULT_NDK_VERSION));
            sdk
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }

        pub(crate) fn str(&self) -> &str {
            self.0.to_str().unwrap()
        }

        /// Creates `<sdk>/ndk/<dir_version>`, with a `source.properties` when
        /// `revision` is given.
        pub(crate) fn install_ndk(&self, dir_version: &str, revision: Option<&str>) {
            let dir = self.0.join("ndk").join(dir_version);
            std::fs::create_dir_all(&dir).unwrap();
            if let Some(rev) = revision {
                std::fs::write(
                    dir.join("source.properties"),
                    format!("Pkg.Desc = Android NDK\nPkg.Revision = {rev}\n"),
                )
                .unwrap();
            }
        }
    }

    impl Drop for TempSdk {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::TempSdk;
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

    fn validate(env: &FakeEnv) -> Validation {
        let runner = FakeProcessRunner::new().with("adb version", ok());
        let ctx = DoctorCtx {
            runner: &runner,
            env,
            is_macos: false,
        };
        AndroidSdkValidator.validate(&ctx)
    }

    fn any_msg(v: &Validation, needle: &str) -> bool {
        v.messages.iter().any(|m| m.contains(needle))
    }

    #[test]
    fn pinned_ndk_under_the_sdk_passes() {
        let sdk = TempSdk::with_default_ndk();
        let result = validate(&FakeEnv::new().set("ANDROID_HOME", sdk.str()));
        assert_eq!(result.status, Status::Pass, "{:?}", result.messages);
        assert!(any_msg(&result, "Android NDK r28"));
    }

    #[test]
    fn sdk_root_may_come_from_android_sdk_root() {
        let sdk = TempSdk::with_default_ndk();
        let result = validate(&FakeEnv::new().set("ANDROID_SDK_ROOT", sdk.str()));
        assert_eq!(result.status, Status::Pass, "{:?}", result.messages);
    }

    #[test]
    fn missing_pinned_ndk_names_the_install_fix() {
        let sdk = TempSdk::new();
        let result = validate(&FakeEnv::new().set("ANDROID_HOME", sdk.str()));
        assert_eq!(result.status, Status::Partial);
        assert!(any_msg(
            &result,
            "sdkmanager --install \"ndk;28.2.13676358\""
        ));
        assert!(any_msg(&result, "FRUST_ANDROID_NDK_VERSION"));
        assert!(!any_msg(&result, "point ANDROID_NDK_HOME"));
    }

    #[test]
    fn override_to_installed_r27_warns_about_alignment() {
        let sdk = TempSdk::with_default_ndk();
        sdk.install_ndk("27.1.12297006", Some("27.1.12297006"));
        let env = FakeEnv::new()
            .set("ANDROID_HOME", sdk.str())
            .set("FRUST_ANDROID_NDK_VERSION", "27.1.12297006");
        let result = validate(&env);
        assert_eq!(result.status, Status::Partial);
        assert!(any_msg(&result, "r27"));
        assert!(any_msg(&result, "16 KB"));
    }

    #[test]
    fn override_to_installed_r29_passes() {
        let sdk = TempSdk::new();
        sdk.install_ndk("29.0.13113456", Some("29.0.13113456"));
        let env = FakeEnv::new()
            .set("ANDROID_HOME", sdk.str())
            .set("FRUST_ANDROID_NDK_VERSION", "29.0.13113456");
        let result = validate(&env);
        assert_eq!(result.status, Status::Pass, "{:?}", result.messages);
        assert!(any_msg(&result, "Android NDK r29"));
    }

    #[test]
    fn override_to_missing_version_names_that_version() {
        let sdk = TempSdk::with_default_ndk();
        let env = FakeEnv::new()
            .set("ANDROID_HOME", sdk.str())
            .set("FRUST_ANDROID_NDK_VERSION", "29.0.13113456");
        let result = validate(&env);
        assert_eq!(result.status, Status::Partial);
        assert!(any_msg(
            &result,
            "sdkmanager --install \"ndk;29.0.13113456\""
        ));
    }

    #[test]
    fn blank_override_falls_back_to_the_default() {
        let sdk = TempSdk::with_default_ndk();
        let env = FakeEnv::new()
            .set("ANDROID_HOME", sdk.str())
            .set("FRUST_ANDROID_NDK_VERSION", "  ");
        assert_eq!(validate(&env).status, Status::Pass);
    }

    #[test]
    fn ndk_home_elsewhere_adds_an_informational_line_without_changing_the_verdict() {
        let sdk = TempSdk::with_default_ndk();
        let env = FakeEnv::new()
            .set("ANDROID_HOME", sdk.str())
            .set("ANDROID_NDK_HOME", "/somewhere/else/ndk");
        let result = validate(&env);
        assert_eq!(result.status, Status::Pass, "{:?}", result.messages);
        assert!(result.messages.iter().any(|m| {
            m.contains("ANDROID_NDK_HOME is /somewhere/else/ndk")
                && m.contains("`frust run` / `frust build` use the SDK copy")
                && m.contains("hot-patch path uses ANDROID_NDK_HOME")
        }));
    }

    #[test]
    fn ndk_home_equal_to_the_resolved_directory_adds_no_line() {
        let sdk = TempSdk::with_default_ndk();
        let resolved = sdk.path().join("ndk").join(DEFAULT_NDK_VERSION);
        let env = FakeEnv::new()
            .set("ANDROID_HOME", sdk.str())
            .set("ANDROID_NDK_HOME", resolved.to_str().unwrap());
        let result = validate(&env);
        assert_eq!(result.status, Status::Pass);
        assert!(!any_msg(&result, "ANDROID_NDK_HOME"));
    }

    #[test]
    fn an_outside_r28_ndk_home_does_not_make_a_missing_sdk_ndk_pass() {
        let sdk = TempSdk::new();
        let outside = TempSdk::new();
        outside.install_ndk("x", Some("28.2.13676358"));
        let home = outside.path().join("ndk").join("x");
        let env = FakeEnv::new()
            .set("ANDROID_HOME", sdk.str())
            .set("ANDROID_NDK_HOME", home.to_str().unwrap());
        let result = validate(&env);
        assert_eq!(result.status, Status::Partial);
        assert!(any_msg(&result, "sdkmanager --install"));
    }

    #[test]
    fn resolved_directory_without_source_properties_is_not_a_plain_pass() {
        let sdk = TempSdk::new();
        sdk.install_ndk(DEFAULT_NDK_VERSION, None);
        let result = validate(&FakeEnv::new().set("ANDROID_HOME", sdk.str()));
        assert_eq!(result.status, Status::Partial);
        assert!(any_msg(&result, "version not verified"));
    }

    #[test]
    fn no_sdk_root_keeps_the_sdk_missing_behaviour() {
        let runner = FakeProcessRunner::new().missing("adb version");
        let env = FakeEnv::new().set("ANDROID_NDK_HOME", "/anywhere");
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let result = AndroidSdkValidator.validate(&ctx);
        assert_eq!(result.status, Status::Fail);
        assert!(any_msg(&result, "Android SDK not found"));
        assert!(!any_msg(&result, "NDK"));
    }

    #[test]
    fn adb_on_path_without_sdk_root_is_partial() {
        let result = validate(&FakeEnv::new());
        assert_eq!(result.status, Status::Partial);
    }

    #[test]
    fn partial_when_sdk_env_set_but_adb_missing() {
        let sdk = TempSdk::with_default_ndk();
        let runner = FakeProcessRunner::new().missing("adb version");
        let env = FakeEnv::new().set("ANDROID_SDK_ROOT", sdk.str());
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        assert_eq!(AndroidSdkValidator.validate(&ctx).status, Status::Partial);
    }

    #[test]
    fn parses_well_formed_revision() {
        assert_eq!(
            parse_ndk_major("Pkg.Desc = Android NDK\nPkg.Revision = 28.2.13676358\n"),
            Some(28)
        );
    }

    #[test]
    fn garbage_revision_yields_none() {
        assert_eq!(parse_ndk_major("Pkg.Revision = banana\n"), None);
        assert_eq!(parse_ndk_major("nothing here"), None);
        assert_eq!(parse_ndk_major(""), None);
    }

    #[test]
    fn package_form_derives_from_the_default_version() {
        assert_eq!(default_ndk_package(), format!("ndk;{DEFAULT_NDK_VERSION}"));
    }

    #[test]
    fn gradle_template_default_matches_the_constant() {
        let template = include_str!("../../templates/app/android.tmpl/app/build.gradle.kts.tmpl");
        assert!(
            template.contains(&format!("?: \"{DEFAULT_NDK_VERSION}\"")),
            "the template's ndkVersionValue default must equal DEFAULT_NDK_VERSION"
        );
    }
}
