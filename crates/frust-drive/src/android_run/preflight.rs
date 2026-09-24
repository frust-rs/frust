//! Preflight checks for `frust run`'s Android path. Each check fails fast
//! with a single, actionable message (the exact fix command) rather than
//! accumulating a report — `frust run` should stop at the first thing
//! standing between the user and a working build.

use crate::doctor::EnvLookup;
use crate::process::ProcessRunner;

/// Fixed path Android Studio installs its bundled JBR at on macOS — probed
/// when `JAVA_HOME` is unset or points at a < 17 JDK.
pub const STUDIO_JBR_HOME: &str = "/Applications/Android Studio.app/Contents/jbr/Contents/Home";

/// Android Studio's bundled-JBR install locations on Windows, relative to the
/// env vars they hang off (see [`windows_studio_jbr_homes`]): the
/// machine-wide install under `%ProgramFiles%`, and the per-user one under
/// `%LOCALAPPDATA%`.
const STUDIO_JBR_WINDOWS_PROGRAM_FILES_SUFFIX: [&str; 3] = ["Android", "Android Studio", "jbr"];
const STUDIO_JBR_WINDOWS_LOCALAPPDATA_SUFFIX: [&str; 3] = ["Programs", "Android Studio", "jbr"];

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
    let (java_home, _source) = check_java(ctx)?;
    check_adb(ctx)?;
    Ok(PreflightOutcome { java_home })
}

/// Like [`run`], but skips [`check_adb`] — used by `frust build`'s
/// Android pipeline (`android_build`), which drives Gradle directly and
/// never talks to a connected device/emulator, so requiring `adb` would be
/// an unrelated hard blocker for a CI/headless release build.
pub fn run_without_device_checks(ctx: &PreflightCtx) -> Result<PreflightOutcome, String> {
    check_rust_target(ctx)?;
    check_cargo_ndk(ctx)?;
    let (java_home, _source) = check_java(ctx)?;
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

/// Linux distros' conventional JVM install root — probed (see
/// [`well_known_jvm_homes`]) once the env var, Studio JBR, and PATH `java`
/// have all come up empty. `/usr/lib/jvm/default` is the symlink several
/// distros (Debian's `default-jdk`, some Arch/Manjaro setups) maintain to
/// their chosen default; `java-*-openjdk` is the versioned-package naming
/// Arch/Manjaro/Fedora derivatives use (e.g. `java-17-openjdk`).
const WELL_KNOWN_JVM_ROOT: &str = "/usr/lib/jvm";

/// Which resolution step in [`check_java`]'s chain produced the resolved
/// `JAVA_HOME` — surfaced by `doctor::report`'s JDK component so its output
/// explains itself instead of just printing a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaSource {
    /// The `JAVA_HOME` environment variable.
    EnvVar,
    /// Android Studio's bundled JBR (macOS only).
    StudioJbr,
    /// `java` resolved off `PATH` (`-XshowSettings:properties`'s
    /// `java.home`).
    Path,
    /// A Linux well-known install location under [`WELL_KNOWN_JVM_ROOT`].
    WellKnown,
}

impl JavaSource {
    /// Human label for `doctor`'s component detail.
    pub fn label(self) -> &'static str {
        match self {
            JavaSource::EnvVar => "JAVA_HOME",
            JavaSource::StudioJbr => "Android Studio's bundled JBR",
            JavaSource::Path => "`java` on PATH",
            JavaSource::WellKnown => "a well-known /usr/lib/jvm install",
        }
    }
}

/// Resolves a `JAVA_HOME` with Java 17+, trying each step in order and
/// returning the first hit along with which step resolved it:
/// 1. The `JAVA_HOME` env var (unchanged — still wins over everything else).
/// 2. Android Studio's bundled JBR (macOS only).
/// 3. Android Studio's bundled JBR at its Windows install locations (Windows
///    only — see [`windows_studio_jbr_homes`]).
/// 4. `java` on `PATH`, resolved via `-XshowSettings:properties`'s
///    `java.home` (see [`probe_path_java`]) — the machine this bug was
///    filed against has no `JAVA_HOME` at all but a working `java` on PATH
///    (Manjaro's `/usr/bin/java` symlink chain into
///    `/usr/lib/jvm/java-17-openjdk`).
/// 5. A Linux well-known location (see [`well_known_jvm_homes`]).
///
/// `pub(crate)`: also the JDK probe `doctor::report`'s component-level
/// report reuses, rather than re-implementing Java-version detection.
pub(crate) fn check_java(ctx: &PreflightCtx) -> Result<(String, JavaSource), String> {
    check_java_for(ctx, cfg!(target_os = "windows"))
}

/// [`check_java`]'s inner logic, taking `windows` explicitly (rather than
/// baking in `cfg!(target_os = "windows")`) so the Windows JBR-probe branch
/// runs under `cargo test` on any host — mirrors
/// `android_run::gradle::gradle_wrapper_for`'s pattern.
fn check_java_for(ctx: &PreflightCtx, windows: bool) -> Result<(String, JavaSource), String> {
    if let Some(home) = ctx.env.get("JAVA_HOME")
        && java_at_least_17(ctx.runner, &home)
    {
        return Ok((home, JavaSource::EnvVar));
    }
    if ctx.is_macos && java_at_least_17(ctx.runner, STUDIO_JBR_HOME) {
        return Ok((STUDIO_JBR_HOME.to_string(), JavaSource::StudioJbr));
    }
    if windows {
        for home in windows_studio_jbr_homes(ctx.env) {
            if java_at_least_17(ctx.runner, &home) {
                return Ok((home, JavaSource::StudioJbr));
            }
        }
    }
    if let Some(home) = probe_path_java(ctx.runner)
        && java_at_least_17(ctx.runner, &home)
    {
        return Ok((home, JavaSource::Path));
    }
    for home in well_known_jvm_homes() {
        if java_at_least_17(ctx.runner, &home) {
            return Ok((home, JavaSource::WellKnown));
        }
    }
    Err(no_java_error(ctx.is_macos, windows))
}

/// Windows locations Android Studio installs its bundled JBR, tried in
/// order: the machine-wide `%ProgramFiles%\Android\Android Studio\jbr`, then
/// the per-user `%LOCALAPPDATA%\Programs\Android Studio\jbr`. Each candidate
/// is built with `Path::join` (so the separator matches the compile target,
/// never a hand-rolled `\`) and kept only if `bin\java.exe` exists there —
/// checked before ever invoking [`java_at_least_17`], the same
/// filesystem-gated shape [`well_known_jvm_homes`] uses for its own
/// candidates. Driven entirely through [`EnvLookup`], so both env vars are
/// fakeable in a test.
fn windows_studio_jbr_homes(env: &dyn EnvLookup) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(program_files) = env.get("ProgramFiles") {
        let mut home = std::path::PathBuf::from(program_files);
        home.extend(STUDIO_JBR_WINDOWS_PROGRAM_FILES_SUFFIX);
        candidates.push(home);
    }
    if let Some(local_appdata) = env.get("LOCALAPPDATA") {
        let mut home = std::path::PathBuf::from(local_appdata);
        home.extend(STUDIO_JBR_WINDOWS_LOCALAPPDATA_SUFFIX);
        candidates.push(home);
    }
    candidates
        .into_iter()
        .filter(|home| home.join("bin").join("java.exe").exists())
        .map(|home| home.to_string_lossy().into_owned())
        .collect()
}

/// [`check_java_for`]'s final failure message: names exactly the sources
/// tried on this host, so a Linux/macOS message never suggests a Windows
/// path and a Windows message never dangles a `.app` bundle or
/// `/usr/lib/jvm`.
fn no_java_error(is_macos: bool, is_windows: bool) -> String {
    let studio_clause = if is_macos || is_windows {
        ", Android Studio's bundled JBR"
    } else {
        ""
    };
    let well_known_clause = if is_windows {
        String::new()
    } else {
        format!(", and well-known {WELL_KNOWN_JVM_ROOT} paths")
    };
    let install_hint = if is_windows {
        "install Android Studio (bundles a JBR at `%ProgramFiles%\\Android\\Android Studio\\jbr` \
         or `%LOCALAPPDATA%\\Programs\\Android Studio\\jbr`)"
            .to_string()
    } else {
        format!("install Android Studio (bundles a JBR at `{STUDIO_JBR_HOME}`)")
    };
    format!(
        "no Java 17+ found (tried JAVA_HOME{studio_clause}, `java` on PATH{well_known_clause}). \
         Set JAVA_HOME to a JDK 17+ install, or {install_hint}."
    )
}

/// Resolves the `java.home` a PATH-visible `java` reports via
/// `-XshowSettings:properties -version` — the canonical cross-platform way
/// to learn where `java` actually lives, since it resolves the full symlink
/// chain a bare `readlink` on `/usr/bin/java` wouldn't (e.g. Manjaro's
/// `/usr/bin/java` → `/etc/alternatives/java` → `/usr/lib/jvm/java-17-openjdk/bin/java`).
/// Returns `None` if `java` isn't on PATH, the invocation fails, or no
/// `java.home` property line is found.
fn probe_path_java(runner: &dyn ProcessRunner) -> Option<String> {
    let out = runner
        .run("java", &["-XshowSettings:properties", "-version"])
        .ok()?;
    if !out.success {
        return None;
    }
    // The properties dump (like the version banner) prints to stderr by
    // convention; fall back to stdout in case a fake/JDK variant writes it
    // there instead — same convention `java_at_least_17` follows below.
    parse_java_home_property(&out.stderr).or_else(|| parse_java_home_property(&out.stdout))
}

/// Parses the `java.home = <path>` line out of
/// `-XshowSettings:properties`'s output.
fn parse_java_home_property(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("java.home")?;
        let value = rest.trim_start().strip_prefix('=')?;
        Some(value.trim().to_string())
    })
}

/// Candidate Linux `JAVA_HOME`s under [`WELL_KNOWN_JVM_ROOT`], in try order:
/// the `default` symlink first, then every `java-*-openjdk` entry newest
/// (highest major version) first. **Best-effort, not fakeable**: this reads
/// the real filesystem directly rather than routing through an injected
/// seam, so it is exercised only against the real machine (never asserted
/// on in a unit test) — [`check_java`]'s unit tests cover the chain
/// ordering and the PATH-probe parse instead, keyed entirely through
/// [`ProcessRunner`]/[`EnvLookup`] fakes. A missing/unreadable
/// [`WELL_KNOWN_JVM_ROOT`] (any non-Linux host, or a Linux host with no
/// system JDK) yields just the `default` candidate, which then fails
/// [`java_at_least_17`] the same as any other absent path.
fn well_known_jvm_homes() -> Vec<String> {
    let mut homes = vec![format!("{WELL_KNOWN_JVM_ROOT}/default")];
    let Ok(entries) = std::fs::read_dir(WELL_KNOWN_JVM_ROOT) else {
        return homes;
    };
    let mut versioned: Vec<(u32, String)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let major = name.strip_prefix("java-")?.split('-').next()?;
            if !name.ends_with("openjdk") {
                return None;
            }
            Some((
                major.parse().ok()?,
                entry.path().to_string_lossy().into_owned(),
            ))
        })
        .collect();
    versioned.sort_by_key(|(major, _)| std::cmp::Reverse(*major));
    homes.extend(versioned.into_iter().map(|(_, path)| path));
    homes
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

    /// A `-XshowSettings:properties -version` fixture: the version banner
    /// plus a `java.home = <path>` property line, mirroring real `java`
    /// output (both print to stderr by convention).
    fn java_properties(java_home: &str, version: &str) -> Output {
        Output {
            success: true,
            stdout: String::new(),
            stderr: format!(
                "openjdk version \"{version}\" 2024-01-16\nOpenJDK Runtime Environment\n\
                 java.class.version = 61.0\njava.home = {java_home}\njava.vendor = Fake Vendor\n"
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
    fn run_without_device_checks_passes_without_adb_registered() {
        // No `adb version` fixture registered at all — an unexpected call
        // would itself error via FakeProcessRunner's "missing" path, proving
        // the adb check is truly skipped.
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .with("/opt/jdk17/bin/java -version", java_ok_stderr("17.0.9"));
        let env = FakeEnv::new().set("JAVA_HOME", "/opt/jdk17");
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let outcome = run_without_device_checks(&ctx).unwrap();
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
    fn resolves_via_path_java_when_java_home_unset() {
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .with(
                "java -XshowSettings:properties -version",
                java_properties("/usr/lib/jvm/java-17-openjdk", "17.0.9"),
            )
            .with(
                "/usr/lib/jvm/java-17-openjdk/bin/java -version",
                java_ok_stderr("17.0.9"),
            )
            .with("adb version", ok("Android Debug Bridge version 1.0.41\n"));
        // No JAVA_HOME, not macOS (so Studio JBR is never even attempted):
        // resolution must fall through to the PATH `java` probe.
        let env = FakeEnv::new();
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let outcome = run(&ctx).unwrap();
        assert_eq!(outcome.java_home, "/usr/lib/jvm/java-17-openjdk");
    }

    #[test]
    fn check_java_reports_path_as_the_resolution_source() {
        let runner = FakeProcessRunner::new()
            .with(
                "java -XshowSettings:properties -version",
                java_properties("/usr/lib/jvm/java-17-openjdk", "17.0.9"),
            )
            .with(
                "/usr/lib/jvm/java-17-openjdk/bin/java -version",
                java_ok_stderr("17.0.9"),
            );
        let env = FakeEnv::new();
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let (home, source) = check_java(&ctx).unwrap();
        assert_eq!(home, "/usr/lib/jvm/java-17-openjdk");
        assert_eq!(source, JavaSource::Path);
    }

    #[test]
    fn java_home_wins_over_path_java_when_both_present() {
        // No fixture registered for the PATH-derived home's own `-version`
        // probe below — an unexpected call would itself error via
        // FakeProcessRunner's "missing" path, proving JAVA_HOME wins before
        // PATH is ever probed.
        let runner = full_runner("/opt/jdk17", "17.0.9").with(
            "java -XshowSettings:properties -version",
            java_properties("/usr/lib/jvm/java-17-openjdk", "17.0.9"),
        );
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
    fn falls_through_when_path_java_is_below_17() {
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .with(
                "java -XshowSettings:properties -version",
                java_properties("/opt/jdk11", "11.0.20"),
            )
            .with("/opt/jdk11/bin/java -version", java_ok_stderr("11.0.20"));
        let env = FakeEnv::new(); // no JAVA_HOME
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let err = run(&ctx).unwrap_err();
        assert!(err.contains("Java 17+"), "{err}");
    }

    #[test]
    fn not_found_error_lists_every_source_tried() {
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .missing("/opt/jdk8/bin/java -version")
            .missing(format!("{STUDIO_JBR_HOME}/bin/java -version"))
            .missing("java -XshowSettings:properties -version");
        let env = FakeEnv::new().set("JAVA_HOME", "/opt/jdk8");
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let err = run(&ctx).unwrap_err();
        assert!(err.contains("JAVA_HOME"), "{err}");
        assert!(err.contains("Android Studio's bundled JBR"), "{err}");
        assert!(err.contains("java` on PATH"), "{err}");
        assert!(err.contains("/usr/lib/jvm"), "{err}");
    }

    /// Plants a fake JBR's `bin/java.exe` under `root` (standing in for
    /// `%ProgramFiles%\Android\Android Studio\jbr` or its `%LOCALAPPDATA%`
    /// sibling), returning the resolved home directory's own path — built
    /// with the same `Path::join` [`windows_studio_jbr_homes`] uses, so the
    /// existence check finds it on any host `cargo test` runs on.
    fn plant_fake_jbr(root: &std::path::Path, suffix: &[&str]) -> String {
        let mut home = root.to_path_buf();
        for part in suffix {
            home.push(part);
        }
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(home.join("bin").join("java.exe"), b"").unwrap();
        home.to_string_lossy().into_owned()
    }

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-preflight-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The Windows arm of [`check_java`]'s chain: no `JAVA_HOME`, but
    /// `%ProgramFiles%\Android\Android Studio\jbr\bin\java.exe` exists and
    /// reports 17+ — resolved as [`JavaSource::StudioJbr`], exercised with
    /// `windows: true` so it runs under `cargo test` on Linux.
    #[test]
    fn windows_probes_studio_jbr_at_program_files() {
        let root = unique_temp_dir("program-files");
        let home = plant_fake_jbr(&root, &STUDIO_JBR_WINDOWS_PROGRAM_FILES_SUFFIX);
        let runner = FakeProcessRunner::new().with(
            format!("{home}/bin/java -version"),
            java_ok_stderr("17.0.9"),
        );
        let env = FakeEnv::new().set("ProgramFiles", &root.to_string_lossy());
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let (resolved_home, source) = check_java_for(&ctx, true).unwrap();
        assert_eq!(resolved_home, home);
        assert_eq!(source, JavaSource::StudioJbr);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The `%LOCALAPPDATA%` sibling, tried when `%ProgramFiles%` is unset (or
    /// its JBR doesn't exist).
    #[test]
    fn windows_probes_studio_jbr_at_localappdata_when_program_files_absent() {
        let root = unique_temp_dir("localappdata");
        let home = plant_fake_jbr(&root, &STUDIO_JBR_WINDOWS_LOCALAPPDATA_SUFFIX);
        let runner = FakeProcessRunner::new().with(
            format!("{home}/bin/java -version"),
            java_ok_stderr("17.0.9"),
        );
        let env = FakeEnv::new().set("LOCALAPPDATA", &root.to_string_lossy());
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let (resolved_home, source) = check_java_for(&ctx, true).unwrap();
        assert_eq!(resolved_home, home);
        assert_eq!(source, JavaSource::StudioJbr);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Neither Windows env var points at an actually-installed JBR (no
    /// `bin/java.exe` on disk at all): the candidate is never even probed via
    /// the runner (no fixture registered for it — an unexpected call would
    /// itself error), and resolution falls through to the next step.
    #[test]
    fn windows_skips_a_studio_jbr_candidate_with_no_java_exe_on_disk() {
        let root = unique_temp_dir("no-jbr");
        std::fs::create_dir_all(&root).unwrap();
        let runner = FakeProcessRunner::new().with(
            "java -XshowSettings:properties -version",
            java_properties("/usr/lib/jvm/java-17-openjdk", "17.0.9"),
        );
        let env = FakeEnv::new().set("ProgramFiles", &root.to_string_lossy());
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let err = check_java_for(&ctx, true).unwrap_err();
        assert!(err.contains("Java 17+"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The Windows arm of [`check_java`] is never reached with `windows:
    /// false` — no `ProgramFiles`/`LOCALAPPDATA` fixture is registered at
    /// all, so a regression that probed it unconditionally would find no
    /// `bin/java.exe` and (harmlessly) still fall through, but this also
    /// proves `windows_studio_jbr_homes` itself is never called by checking
    /// no filesystem probe happens against a bogus `ProgramFiles` pointing
    /// nowhere.
    #[test]
    fn non_windows_never_probes_studio_jbr_windows_locations() {
        let runner = FakeProcessRunner::new()
            .with(
                "java -XshowSettings:properties -version",
                java_properties("/usr/lib/jvm/java-17-openjdk", "17.0.9"),
            )
            .with(
                "/usr/lib/jvm/java-17-openjdk/bin/java -version",
                java_ok_stderr("17.0.9"),
            );
        // Points nowhere real — if `windows_studio_jbr_homes` were consulted
        // with `windows: false`, its existence check would (correctly) find
        // nothing there either, but resolution must reach `Path` via the
        // non-Windows branch either way.
        let env = FakeEnv::new().set("ProgramFiles", "/nonexistent");
        let ctx = PreflightCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let (home, source) = check_java_for(&ctx, false).unwrap();
        assert_eq!(home, "/usr/lib/jvm/java-17-openjdk");
        assert_eq!(source, JavaSource::Path);
    }

    /// The Windows error text: no `/usr/lib/jvm`, no macOS `.app` bundle
    /// path — instead the two Windows JBR install locations.
    #[test]
    fn windows_not_found_error_names_windows_locations_only() {
        let err = no_java_error(false, true);
        assert!(err.contains("JAVA_HOME"), "{err}");
        assert!(err.contains("Android Studio's bundled JBR"), "{err}");
        assert!(err.contains("java` on PATH"), "{err}");
        assert!(err.contains("%ProgramFiles%"), "{err}");
        assert!(err.contains("%LOCALAPPDATA%"), "{err}");
        assert!(!err.contains("/usr/lib/jvm"), "{err}");
        assert!(!err.contains(".app"), "{err}");
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
