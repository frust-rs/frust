//! Preflight checks for `forgekit run`'s Android path (spec §12.4 step 3).
//! Each check fails fast with a single, actionable message (the exact fix
//! command) rather than accumulating a report — `forgekit run` should stop
//! at the first thing standing between the user and a working build.

use crate::doctor::EnvLookup;
use crate::process::ProcessRunner;

/// Fixed path Android Studio installs its bundled JBR at on macOS — probed
/// when `JAVA_HOME` is unset or points at a < 17 JDK.
pub const STUDIO_JBR_HOME: &str = "/Applications/Android Studio.app/Contents/jbr/Contents/Home";

pub struct PreflightCtx<'a> {
    pub runner: &'a dyn ProcessRunner,
    pub env: &'a dyn EnvLookup,
    pub is_macos: bool,
}

/// Resolved values downstream steps need.
#[derive(Debug)]
pub struct PreflightOutcome {
    /// `JAVA_HOME` to export for the `./gradlew` child process.
    pub java_home: String,
}

/// Runs every check in order, stopping at (and returning) the first
/// failure's actionable message.
pub fn run(ctx: &PreflightCtx) -> Result<PreflightOutcome, String> {
    check_rust_target(ctx)?;
    check_cargo_ndk(ctx)?;
    let java_home = check_java(ctx)?;
    check_adb(ctx)?;
    Ok(PreflightOutcome { java_home })
}

fn check_rust_target(ctx: &PreflightCtx) -> Result<(), String> {
    match ctx.runner.run("rustup", &["target", "list", "--installed"]) {
        Ok(out) if out.success && has_target(&out.stdout, "aarch64-linux-android") => Ok(()),
        _ => Err(
            "missing Rust target aarch64-linux-android. Run: rustup target add aarch64-linux-android"
                .to_string(),
        ),
    }
}

fn has_target(installed: &str, target: &str) -> bool {
    installed.lines().any(|line| line.trim() == target)
}

fn check_cargo_ndk(ctx: &PreflightCtx) -> Result<(), String> {
    match ctx.runner.run("cargo", &["ndk", "--version"]) {
        Ok(out) if out.success => Ok(()),
        _ => Err("cargo-ndk not found. Run: cargo install cargo-ndk".to_string()),
    }
}

/// Resolves a `JAVA_HOME` with Java 17+: prefers the env var, falls back
/// (macOS only) to Android Studio's bundled JBR.
fn check_java(ctx: &PreflightCtx) -> Result<String, String> {
    if let Some(home) = ctx.env.get("JAVA_HOME")
        && java_at_least_17(ctx.runner, &home)
    {
        return Ok(home);
    }
    if ctx.is_macos && java_at_least_17(ctx.runner, STUDIO_JBR_HOME) {
        return Ok(STUDIO_JBR_HOME.to_string());
    }
    Err(format!(
        "no Java 17+ found. Set JAVA_HOME to a JDK 17+ install, or install Android Studio \
         (bundles a JBR at `{STUDIO_JBR_HOME}`)."
    ))
}

fn java_at_least_17(runner: &dyn ProcessRunner, home: &str) -> bool {
    let java_bin = format!("{home}/bin/java");
    let Ok(out) = runner.run(&java_bin, &["-version"]) else {
        return false;
    };
    if !out.success {
        return false;
    }
    // `java -version` writes to stderr by convention; fall back to stdout
    // in case a fake/JDK variant writes it there instead.
    parse_java_major_version(&out.stderr)
        .or_else(|| parse_java_major_version(&out.stdout))
        .is_some_and(|major| major >= 17)
}

/// Parses the major version out of `java -version`'s `version "X[.Y.Z]"`
/// line, handling both the modern (`"17.0.9"` → 17) and legacy
/// (`"1.8.0_301"` → 8) numbering schemes.
fn parse_java_major_version(text: &str) -> Option<u32> {
    let start = text.find("version \"")? + "version \"".len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    let version = &rest[..end];
    let mut parts = version.split('.');
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

fn check_adb(ctx: &PreflightCtx) -> Result<(), String> {
    match ctx.runner.run("adb", &["version"]) {
        Ok(out) if out.success => Ok(()),
        _ => Err("adb not found on PATH. Install Android SDK platform-tools.".to_string()),
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

    fn java_ok_stderr(version: &str) -> Output {
        Output {
            success: true,
            stdout: String::new(),
            stderr: format!(
                "openjdk version \"{version}\" 2024-01-16\nOpenJDK Runtime Environment\n"
            ),
        }
    }

    fn full_runner(java_home: &str, java_version: &str) -> FakeProcessRunner {
        FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .with(
                format!("{java_home}/bin/java -version"),
                java_ok_stderr(java_version),
            )
            .with("adb version", ok("Android Debug Bridge version 1.0.41\n"))
    }

    #[test]
    fn passes_when_everything_present() {
        let runner = full_runner("/opt/jdk17", "17.0.9");
        let env = FakeEnv::new().set("JAVA_HOME", "/opt/jdk17");
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let outcome = run(&ctx).unwrap();
        assert_eq!(outcome.java_home, "/opt/jdk17");
    }

    #[test]
    fn fails_with_actionable_message_when_target_missing() {
        let runner = FakeProcessRunner::new().with(
            "rustup target list --installed",
            ok("x86_64-apple-darwin\n"),
        );
        let env = FakeEnv::new();
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let err = run(&ctx).unwrap_err();
        assert!(
            err.contains("rustup target add aarch64-linux-android"),
            "{err}"
        );
    }

    #[test]
    fn fails_when_cargo_ndk_missing() {
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\n"),
            )
            .missing("cargo ndk --version");
        let env = FakeEnv::new();
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let err = run(&ctx).unwrap_err();
        assert!(err.contains("cargo install cargo-ndk"), "{err}");
    }

    #[test]
    fn falls_back_to_studio_jbr_when_java_home_unset() {
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .with(
                format!("{STUDIO_JBR_HOME}/bin/java -version"),
                java_ok_stderr("17.0.9"),
            )
            .with("adb version", ok("Android Debug Bridge version 1.0.41\n"));
        let env = FakeEnv::new(); // no JAVA_HOME
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let outcome = run(&ctx).unwrap();
        assert_eq!(outcome.java_home, STUDIO_JBR_HOME);
    }

    #[test]
    fn falls_back_to_studio_jbr_when_java_home_java_is_too_old() {
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .with("/opt/jdk8/bin/java -version", java_ok_stderr("1.8.0_301"))
            .with(
                format!("{STUDIO_JBR_HOME}/bin/java -version"),
                java_ok_stderr("17.0.9"),
            )
            .with("adb version", ok("Android Debug Bridge version 1.0.41\n"));
        let env = FakeEnv::new().set("JAVA_HOME", "/opt/jdk8");
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let outcome = run(&ctx).unwrap();
        assert_eq!(outcome.java_home, STUDIO_JBR_HOME);
    }

    #[test]
    fn errs_when_no_java_17_anywhere() {
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .missing("/opt/jdk8/bin/java -version")
            .missing(format!("{STUDIO_JBR_HOME}/bin/java -version"));
        let env = FakeEnv::new().set("JAVA_HOME", "/opt/jdk8");
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let err = run(&ctx).unwrap_err();
        assert!(err.contains("Java 17+"), "{err}");
    }

    #[test]
    fn does_not_probe_studio_jbr_off_macos() {
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"));
        // No JAVA_HOME and not macOS: must fail without ever calling the
        // Studio JBR path (no fixture registered for it — an unexpected
        // call would itself error via FakeProcessRunner's "missing" path).
        let env = FakeEnv::new();
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let err = run(&ctx).unwrap_err();
        assert!(err.contains("Java 17+"), "{err}");
    }

    #[test]
    fn fails_when_adb_missing() {
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .with("/opt/jdk17/bin/java -version", java_ok_stderr("17.0.9"))
            .missing("adb version");
        let env = FakeEnv::new().set("JAVA_HOME", "/opt/jdk17");
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let err = run(&ctx).unwrap_err();
        assert!(err.contains("adb not found"), "{err}");
    }
}
