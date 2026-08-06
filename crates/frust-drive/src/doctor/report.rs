//! Component-level toolchain report: a structured, per-component
//! breakdown of `doctor`'s checks, grouped by area
//! (Prerequisites / Android / iOS / Desktop), each component carrying a
//! human summary and zero-or-more structured, possibly-runnable fix
//! commands — the shape the TUI bootstrap wizard (fdemon's InstallWizard is
//! a pattern source only, BSL-1.1) and titlebar toolchain chip consume.
//!
//! Built entirely on top of the existing flat [`Validator`] set (reused via
//! their `validate` calls, never re-shelled) plus the JDK/signing probes
//! `android_run::preflight`/`ios_build::team` already own — the CLI's
//! `frust doctor` output and the [`Validator`]/[`Status`] surface it uses
//! stay completely untouched by this module.

use std::collections::HashSet;

use crate::android_run::preflight::{self, PreflightCtx};
use crate::ios_build::team;

use super::mobile_targets::{self, ANDROID_TARGETS, IOS_TARGETS};
use super::{
    AndroidSdkValidator, CargoNdkValidator, DoctorCtx, RustToolchainValidator, Status, Validation,
    Validator, XcodeValidator,
};

/// Per-component/per-area/whole-report status: a tri-state distinct from
/// [`Status`] (which stays the CLI's flat pass/partial/fail surface) —
/// mirrors fdemon's InstallWizard rollup vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentStatus {
    Ok,
    Partial,
    Missing,
}

impl From<Status> for ComponentStatus {
    fn from(status: Status) -> Self {
        match status {
            Status::Pass => ComponentStatus::Ok,
            Status::Partial => ComponentStatus::Partial,
            Status::Fail => ComponentStatus::Missing,
        }
    }
}

/// A structured, single (never chained) fix step for one component.
/// `auto_runnable` is `false` for privileged/system installs (Xcode,
/// Android Studio, a JDK) — those carry a `doc_link` instead of a command
/// the wizard could run as a supervised session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixCommand {
    /// Display string shown next to the wizard's action.
    pub display: String,
    /// Program to run via `ProcessRunner`/a supervised session. Empty when
    /// `auto_runnable` is `false` — there's nothing to spawn.
    pub program: String,
    pub args: Vec<String>,
    pub auto_runnable: bool,
    pub doc_link: Option<String>,
}

impl FixCommand {
    fn runnable(display: impl Into<String>, program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            display: display.into(),
            program: program.into(),
            args,
            auto_runnable: true,
            doc_link: None,
        }
    }

    fn guidance(display: impl Into<String>, doc_link: Option<&str>) -> Self {
        Self {
            display: display.into(),
            program: String::new(),
            args: Vec::new(),
            auto_runnable: false,
            doc_link: doc_link.map(str::to_string),
        }
    }
}

/// One toolchain component's status, human summary, and fix command(s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub status: ComponentStatus,
    pub summary: String,
    pub fix_commands: Vec<FixCommand>,
}

/// A named grouping of components (Prerequisites / Android / iOS /
/// Desktop).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Area {
    pub name: String,
    pub components: Vec<Component>,
}

impl Area {
    /// Worst status among this area's components (`Ok` for an empty area).
    pub fn status(&self) -> ComponentStatus {
        worst(self.components.iter().map(|c| c.status))
    }
}

/// The area name gating the whole report's rollup — a `Missing` component
/// here blocks; every other area's gaps are non-blocking (fdemon's rollup
/// model).
pub const CORE_AREA: &str = "Prerequisites";

/// The full component-level report; the titlebar toolchain chip and
/// bootstrap wizard both consume this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    pub areas: Vec<Area>,
}

impl DoctorReport {
    /// Whole-report rollup: the core
    /// (Prerequisites) area gates — its `Missing` propagates straight
    /// through — while every other area's `Missing`/`Partial` degrades the
    /// rollup only as far as `Partial`, never blocking on a platform gap.
    pub fn rollup(&self) -> ComponentStatus {
        let core = self
            .areas
            .iter()
            .find(|area| area.name == CORE_AREA)
            .map(Area::status)
            .unwrap_or(ComponentStatus::Ok);
        if core == ComponentStatus::Missing {
            return ComponentStatus::Missing;
        }
        let other_gap = self
            .areas
            .iter()
            .filter(|area| area.name != CORE_AREA)
            .any(|area| area.status() != ComponentStatus::Ok);
        if core == ComponentStatus::Partial || other_gap {
            ComponentStatus::Partial
        } else {
            ComponentStatus::Ok
        }
    }
}

fn worst(statuses: impl Iterator<Item = ComponentStatus>) -> ComponentStatus {
    statuses.fold(ComponentStatus::Ok, |acc, status| match (acc, status) {
        (ComponentStatus::Missing, _) | (_, ComponentStatus::Missing) => ComponentStatus::Missing,
        (ComponentStatus::Partial, _) | (_, ComponentStatus::Partial) => ComponentStatus::Partial,
        _ => ComponentStatus::Ok,
    })
}

/// Builds the full component-level report against `ctx`, reusing every
/// existing validator's/preflight check's probe logic instead of
/// re-shelling: Prerequisites always runs; Android always runs
/// (cross-compiled from any host, like
/// [`MobileTargetsValidator`]); iOS runs only on a macOS host (`xcodebuild`/
/// the simulator toolchain don't exist elsewhere); Desktop is always `Ok`.
pub fn build_report(ctx: &DoctorCtx) -> DoctorReport {
    let mut areas = vec![prerequisites_area(ctx), android_area(ctx)];
    if ctx.is_macos {
        areas.push(ios_area(ctx));
    }
    areas.push(desktop_area());
    DoctorReport { areas }
}

fn prerequisites_area(ctx: &DoctorCtx) -> Area {
    let validation = RustToolchainValidator.validate(ctx);
    Area {
        name: CORE_AREA.to_string(),
        components: vec![Component {
            name: "Rust toolchain".to_string(),
            status: validation.status.into(),
            fix_commands: rust_toolchain_fixes(&validation),
            summary: validation.messages.join("; "),
        }],
    }
}

fn rust_toolchain_fixes(validation: &Validation) -> Vec<FixCommand> {
    match validation.status {
        Status::Pass | Status::Partial => Vec::new(),
        Status::Fail => {
            if validation.messages.iter().any(|m| m.contains("rustup.rs")) {
                vec![FixCommand::guidance(
                    "Install Rust via rustup",
                    Some("https://rustup.rs"),
                )]
            } else {
                vec![FixCommand::runnable(
                    "Update Rust toolchain",
                    "rustup",
                    vec!["update".to_string()],
                )]
            }
        }
    }
}

fn android_area(ctx: &DoctorCtx) -> Area {
    Area {
        name: "Android".to_string(),
        components: vec![
            rustup_targets_component(ctx, "Android Rust targets", ANDROID_TARGETS),
            cargo_ndk_component(ctx),
            jdk_component(ctx),
            android_sdk_component(ctx),
        ],
    }
}

fn ios_area(ctx: &DoctorCtx) -> Area {
    Area {
        name: "iOS".to_string(),
        components: vec![
            xcode_component(ctx),
            rustup_targets_component(ctx, "iOS Rust targets", IOS_TARGETS),
            signing_component(ctx),
        ],
    }
}

fn desktop_area() -> Area {
    Area {
        name: "Desktop".to_string(),
        components: vec![Component {
            name: "Desktop preview".to_string(),
            status: ComponentStatus::Ok,
            summary: "always available (`cargo run`)".to_string(),
            fix_commands: Vec::new(),
        }],
    }
}

/// Shared by both Android's and iOS's Rust-target components — `required`
/// is [`ANDROID_TARGETS`] or [`IOS_TARGETS`], probed once each via
/// [`mobile_targets::probe_installed_targets`] (the same helper
/// [`MobileTargetsValidator`] uses).
fn rustup_targets_component(ctx: &DoctorCtx, name: &str, required: &[&str]) -> Component {
    match mobile_targets::probe_installed_targets(ctx) {
        Some(installed) => {
            let missing = missing_targets(required, &installed);
            if missing.is_empty() {
                Component {
                    name: name.to_string(),
                    status: ComponentStatus::Ok,
                    summary: "all required targets installed".to_string(),
                    fix_commands: Vec::new(),
                }
            } else {
                let mut args = vec!["target".to_string(), "add".to_string()];
                args.extend(missing.iter().map(|t| t.to_string()));
                Component {
                    name: name.to_string(),
                    status: ComponentStatus::Partial,
                    summary: format!("missing: {}", missing.join(", ")),
                    fix_commands: vec![FixCommand::runnable(
                        format!("rustup target add {}", missing.join(" ")),
                        "rustup",
                        args,
                    )],
                }
            }
        }
        None => Component {
            name: name.to_string(),
            status: ComponentStatus::Missing,
            summary: "rustup not found".to_string(),
            fix_commands: vec![FixCommand::guidance(
                "Install Rust via rustup",
                Some("https://rustup.rs"),
            )],
        },
    }
}

fn missing_targets<'a>(required: &[&'a str], installed: &HashSet<String>) -> Vec<&'a str> {
    required
        .iter()
        .copied()
        .filter(|t| !installed.contains(*t))
        .collect()
}

fn cargo_ndk_component(ctx: &DoctorCtx) -> Component {
    let validation = CargoNdkValidator.validate(ctx);
    let status: ComponentStatus = validation.status.into();
    let fix_commands = match status {
        ComponentStatus::Ok => Vec::new(),
        _ => vec![FixCommand::runnable(
            "cargo install cargo-ndk",
            "cargo",
            vec!["install".to_string(), "cargo-ndk".to_string()],
        )],
    };
    Component {
        name: "cargo-ndk".to_string(),
        status,
        summary: validation.messages.join("; "),
        fix_commands,
    }
}

fn jdk_component(ctx: &DoctorCtx) -> Component {
    let preflight_ctx = PreflightCtx {
        runner: ctx.runner,
        env: ctx.env,
        is_macos: ctx.is_macos,
    };
    match preflight::check_java(&preflight_ctx) {
        Ok(home) => Component {
            name: "JDK".to_string(),
            status: ComponentStatus::Ok,
            summary: format!("Java 17+ at {home}"),
            fix_commands: Vec::new(),
        },
        Err(message) => Component {
            name: "JDK".to_string(),
            status: ComponentStatus::Missing,
            summary: message,
            fix_commands: vec![FixCommand::guidance(
                "Install a JDK 17+ (or Android Studio, which bundles one)",
                Some("https://developer.android.com/studio"),
            )],
        },
    }
}

fn android_sdk_component(ctx: &DoctorCtx) -> Component {
    let validation = AndroidSdkValidator.validate(ctx);
    let status: ComponentStatus = validation.status.into();
    let fix_commands = match status {
        ComponentStatus::Ok => Vec::new(),
        _ => vec![FixCommand::guidance(
            "Install Android Studio (SDK + NDK) and set ANDROID_HOME/ANDROID_NDK_HOME",
            Some("https://developer.android.com/studio"),
        )],
    };
    Component {
        name: "Android SDK/NDK".to_string(),
        status,
        summary: validation.messages.join("; "),
        fix_commands,
    }
}

fn xcode_component(ctx: &DoctorCtx) -> Component {
    let validation = XcodeValidator.validate(ctx);
    let status: ComponentStatus = validation.status.into();
    let fix_commands = match status {
        ComponentStatus::Ok => Vec::new(),
        _ => vec![FixCommand::guidance(
            "Install Xcode from the App Store",
            Some("https://apps.apple.com/app/xcode/id497799835"),
        )],
    };
    Component {
        name: "Xcode".to_string(),
        status,
        summary: validation.messages.join("; "),
        fix_commands,
    }
}

/// Codesigning-identity probe (macOS only, called only from [`ios_area`]),
/// reusing `ios_build::team::detect` rather than re-parsing
/// `security find-identity` output. A signed build only matters for a
/// physical-device/release run — no identity is `Partial`, not `Missing`
/// (unsigned Simulator runs still work), so it never gates the rollup.
fn signing_component(ctx: &DoctorCtx) -> Component {
    match team::detect(ctx.runner) {
        Ok(teams) if !teams.is_empty() => Component {
            name: "Signing".to_string(),
            status: ComponentStatus::Ok,
            summary: format!(
                "{} codesigning {} found",
                teams.len(),
                if teams.len() == 1 {
                    "identity"
                } else {
                    "identities"
                }
            ),
            fix_commands: Vec::new(),
        },
        Ok(_) => Component {
            name: "Signing".to_string(),
            status: ComponentStatus::Partial,
            summary: "no codesigning identity found (Simulator/unsigned builds still work)"
                .to_string(),
            fix_commands: vec![FixCommand::guidance(
                "Add your Apple ID in Xcode → Settings → Accounts",
                None,
            )],
        },
        Err(_) => Component {
            name: "Signing".to_string(),
            status: ComponentStatus::Partial,
            summary: "could not probe codesigning identities (`security find-identity` failed)"
                .to_string(),
            fix_commands: Vec::new(),
        },
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

    fn ok_stderr(stderr: &str) -> Output {
        Output {
            success: true,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    /// A fully green macOS host: every probe registered with a passing
    /// canned response.
    fn all_green_runner() -> FakeProcessRunner {
        FakeProcessRunner::new()
            .with(
                "rustc --version",
                ok("rustc 1.91.1 (ed61e7d7e 2025-11-07)\n"),
            )
            .with("cargo --version", ok("cargo 1.91.1\n"))
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\narmv7-linux-androideabi\nx86_64-linux-android\naarch64-apple-ios\naarch64-apple-ios-sim\n"),
            )
            .with(
                "cargo ndk --version",
                ok("cargo-ndk 3.5.4\n"),
            )
            .with(
                "/opt/jdk17/bin/java -version",
                ok_stderr("openjdk version \"17.0.9\" 2024-01-16\n"),
            )
            .with("adb version", ok("Android Debug Bridge version 1.0.41\n"))
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "xcodebuild -version",
                ok("Xcode 15.4\nBuild version 15F31d\n"),
            )
            .with(
                "security find-identity -v -p codesigning",
                ok("  1) ABCDEF1234 \"Apple Development: dev@example.com (TEAMID1234)\"\n     1 valid identities found\n"),
            )
    }

    fn all_green_env() -> FakeEnv {
        FakeEnv::new()
            .set("JAVA_HOME", "/opt/jdk17")
            .set("ANDROID_HOME", "/sdk")
            .set("ANDROID_NDK_HOME", "/sdk/ndk/26.1.10909125")
    }

    #[test]
    fn all_green_report_rolls_up_ok() {
        let runner = all_green_runner();
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let report = build_report(&ctx);
        assert_eq!(report.rollup(), ComponentStatus::Ok);
        for area in &report.areas {
            for component in &area.components {
                assert_eq!(
                    component.status,
                    ComponentStatus::Ok,
                    "{}/{} not Ok: {}",
                    area.name,
                    component.name,
                    component.summary
                );
                assert!(component.fix_commands.is_empty());
            }
        }
        assert!(report.areas.iter().any(|a| a.name == "iOS"));
    }

    #[test]
    fn missing_cargo_ndk_emits_auto_runnable_fix() {
        let runner = all_green_runner().missing("cargo ndk --version");
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let report = build_report(&ctx);
        let android = report.areas.iter().find(|a| a.name == "Android").unwrap();
        let ndk = android
            .components
            .iter()
            .find(|c| c.name == "cargo-ndk")
            .unwrap();
        assert_eq!(ndk.status, ComponentStatus::Missing);
        assert_eq!(ndk.fix_commands.len(), 1);
        let fix = &ndk.fix_commands[0];
        assert!(fix.auto_runnable);
        assert_eq!(fix.program, "cargo");
        assert_eq!(fix.args, vec!["install", "cargo-ndk"]);
        // Core (Prerequisites) is untouched — rollup degrades only to
        // Partial, a platform gap never blocks.
        assert_eq!(report.rollup(), ComponentStatus::Partial);
    }

    #[test]
    fn missing_xcode_on_macos_is_guidance_only() {
        let runner = all_green_runner().missing("xcode-select -p");
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let report = build_report(&ctx);
        let ios = report.areas.iter().find(|a| a.name == "iOS").unwrap();
        let xcode = ios.components.iter().find(|c| c.name == "Xcode").unwrap();
        assert_eq!(xcode.status, ComponentStatus::Missing);
        assert_eq!(xcode.fix_commands.len(), 1);
        let fix = &xcode.fix_commands[0];
        assert!(!fix.auto_runnable);
        assert!(fix.program.is_empty());
        assert!(fix.doc_link.is_some());
        assert_eq!(report.rollup(), ComponentStatus::Partial);
    }

    #[test]
    fn non_macos_host_has_no_ios_section() {
        // Android-only fixtures; a call the iOS section would make (e.g.
        // `xcode-select -p`) has no registered response, so an unexpected
        // call would itself error via FakeProcessRunner's "missing" path.
        let runner = FakeProcessRunner::new()
            .with(
                "rustc --version",
                ok("rustc 1.91.1 (ed61e7d7e 2025-11-07)\n"),
            )
            .with("cargo --version", ok("cargo 1.91.1\n"))
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\narmv7-linux-androideabi\nx86_64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .with(
                "/opt/jdk17/bin/java -version",
                ok_stderr("openjdk version \"17.0.9\" 2024-01-16\n"),
            )
            .with("adb version", ok("Android Debug Bridge version 1.0.41\n"));
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let report = build_report(&ctx);
        assert!(report.areas.iter().all(|a| a.name != "iOS"));
        assert!(report.areas.iter().any(|a| a.name == "Desktop"));
        assert_eq!(report.rollup(), ComponentStatus::Ok);
    }

    #[test]
    fn env_var_gap_reports_partial_android_sdk_with_guidance_fix() {
        // ANDROID_NDK_HOME unset: AndroidSdkValidator degrades to Partial.
        let runner = all_green_runner();
        let env = FakeEnv::new()
            .set("JAVA_HOME", "/opt/jdk17")
            .set("ANDROID_HOME", "/sdk");
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let report = build_report(&ctx);
        let android = report.areas.iter().find(|a| a.name == "Android").unwrap();
        let sdk = android
            .components
            .iter()
            .find(|c| c.name == "Android SDK/NDK")
            .unwrap();
        assert_eq!(sdk.status, ComponentStatus::Partial);
        assert_eq!(sdk.fix_commands.len(), 1);
        assert!(!sdk.fix_commands[0].auto_runnable);
        assert_eq!(report.rollup(), ComponentStatus::Partial);
    }

    #[test]
    fn missing_rust_targets_emit_auto_runnable_rustup_fix() {
        let runner = all_green_runner().with(
            "rustup target list --installed",
            ok("aarch64-linux-android\n"),
        );
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let report = build_report(&ctx);
        let android = report.areas.iter().find(|a| a.name == "Android").unwrap();
        let targets = android
            .components
            .iter()
            .find(|c| c.name == "Android Rust targets")
            .unwrap();
        assert_eq!(targets.status, ComponentStatus::Partial);
        let fix = &targets.fix_commands[0];
        assert!(fix.auto_runnable);
        assert_eq!(fix.program, "rustup");
        assert!(fix.args.contains(&"armv7-linux-androideabi".to_string()));
    }

    #[test]
    fn missing_rustc_gates_rollup_to_missing() {
        let runner = all_green_runner().missing("rustc --version");
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let report = build_report(&ctx);
        // Core gates: rollup is Missing, not merely Partial, even though
        // every platform area is still green.
        assert_eq!(report.rollup(), ComponentStatus::Missing);
        let prereqs = report.areas.iter().find(|a| a.name == CORE_AREA).unwrap();
        assert_eq!(prereqs.status(), ComponentStatus::Missing);
    }

    #[test]
    fn no_signing_identity_is_partial_not_missing() {
        let runner = all_green_runner().with(
            "security find-identity -v -p codesigning",
            ok("0 valid identities found\n"),
        );
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let report = build_report(&ctx);
        let ios = report.areas.iter().find(|a| a.name == "iOS").unwrap();
        let signing = ios.components.iter().find(|c| c.name == "Signing").unwrap();
        assert_eq!(signing.status, ComponentStatus::Partial);
        assert_eq!(report.rollup(), ComponentStatus::Partial);
    }
}
