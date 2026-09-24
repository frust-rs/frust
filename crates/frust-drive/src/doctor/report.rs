//! Component-level toolchain report: a structured, per-component
//! breakdown of `doctor`'s checks, grouped by area
//! (Prerequisites / Android / iOS / Desktop / Web), each component carrying a
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
    Validator, WasmBindgenCliValidator, WasmOptValidator, WasmTargetValidator, XcodeValidator,
    wasm_bindgen_cli, wasm_opt, wasm_target,
};
use crate::build_info::WASM_TARGET_TRIPLE;
use crate::web_build::WASM_BINDGEN_PINNED;

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
/// the simulator toolchain don't exist elsewhere); Desktop is always `Ok`;
/// Web always runs (a browser build cross-compiles from any host, like
/// Android).
pub fn build_report(ctx: &DoctorCtx) -> DoctorReport {
    let mut areas = vec![prerequisites_area(ctx), android_area(ctx)];
    if ctx.is_macos {
        areas.push(ios_area(ctx));
    }
    areas.push(desktop_area());
    areas.push(web_area(ctx));
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

/// The browser toolchain, from the three web validators' own `validate`
/// results (never re-shelled). Non-core like every platform area, and its
/// components are `Partial` at worst by construction — the validators never
/// return [`Status::Fail`], because a host that will never build for the
/// browser is not a broken host — so a completely absent web toolchain
/// degrades [`DoctorReport::rollup`] to `Partial` and nothing worse.
fn web_area(ctx: &DoctorCtx) -> Area {
    Area {
        name: "Web".to_string(),
        components: vec![
            wasm_target_component(ctx),
            wasm_bindgen_component(ctx),
            wasm_opt_component(ctx),
        ],
    }
}

fn wasm_target_component(ctx: &DoctorCtx) -> Component {
    let validation = WasmTargetValidator.validate(ctx);
    let status: ComponentStatus = validation.status.into();
    let fix_commands = match status {
        ComponentStatus::Ok => Vec::new(),
        _ => vec![FixCommand::runnable(
            wasm_target::add_target_command(),
            "rustup",
            vec![
                "target".to_string(),
                "add".to_string(),
                WASM_TARGET_TRIPLE.to_string(),
            ],
        )],
    };
    Component {
        name: WasmTargetValidator.name().to_string(),
        status,
        summary: validation.messages.join("; "),
        fix_commands,
    }
}

/// The fix names [`WASM_BINDGEN_PINNED`] — the framework's pin — because this
/// report has no project in hand; a project pinning something else is checked
/// against its own pin by `crate::web_build::preflight`, and the validator's
/// message says which wins.
fn wasm_bindgen_component(ctx: &DoctorCtx) -> Component {
    let validation = WasmBindgenCliValidator.validate(ctx);
    let status: ComponentStatus = validation.status.into();
    let fix_commands = match status {
        ComponentStatus::Ok => Vec::new(),
        _ => vec![FixCommand::runnable(
            wasm_bindgen_cli::install_command(WASM_BINDGEN_PINNED),
            "cargo",
            vec![
                "install".to_string(),
                "-f".to_string(),
                "wasm-bindgen-cli".to_string(),
                "--version".to_string(),
                WASM_BINDGEN_PINNED.to_string(),
            ],
        )],
    };
    Component {
        name: WasmBindgenCliValidator.name().to_string(),
        status,
        summary: validation.messages.join("; "),
        fix_commands,
    }
}

/// Guidance, not a command: binaryen ships as a platform package (brew, apt,
/// a release archive), so there is no one invocation a wizard could run.
fn wasm_opt_component(ctx: &DoctorCtx) -> Component {
    let validation = WasmOptValidator.validate(ctx);
    let status: ComponentStatus = validation.status.into();
    let fix_commands = match status {
        ComponentStatus::Ok => Vec::new(),
        _ => vec![FixCommand::guidance(
            wasm_opt::INSTALL_GUIDANCE,
            Some(wasm_opt::INSTALL_DOC_LINK),
        )],
    };
    Component {
        name: WasmOptValidator.name().to_string(),
        status,
        summary: validation.messages.join("; "),
        fix_commands,
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
        Ok((home, source)) => Component {
            name: "JDK".to_string(),
            status: ComponentStatus::Ok,
            summary: format!("Java 17+ at {home} (via {})", source.label()),
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
                ok("aarch64-linux-android\narmv7-linux-androideabi\nx86_64-linux-android\naarch64-apple-ios\naarch64-apple-ios-sim\nwasm32-unknown-unknown\n"),
            )
            .with(
                "wasm-bindgen --version",
                ok(&format!("wasm-bindgen {WASM_BINDGEN_PINNED}\n")),
            )
            .with("wasm-opt --version", ok("wasm-opt version 130\n"))
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

    /// Direct, area-gating-independent regression coverage for the
    /// host-honest doctor fix: even called head-on (bypassing
    /// `build_report`'s own `is_macos` gate on the whole iOS area, see
    /// `non_macos_host_has_no_ios_section` below for that path), the Xcode
    /// component must never render as `Ok` on a non-macOS host — the CLI's
    /// `print_results` only shows a component's message inline without `-v`
    /// when the status isn't `Ok`/`Pass`, and a checkmark for a component
    /// that can't exist on this host is exactly the bug this closes.
    #[test]
    fn xcode_component_is_not_ok_on_a_non_macos_host() {
        let runner = FakeProcessRunner::new();
        let env = FakeEnv::new();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let xcode = xcode_component(&ctx);
        assert_ne!(xcode.status, ComponentStatus::Ok);
        assert!(xcode.summary.contains("skipped (not macOS)"));
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
                ok("aarch64-linux-android\narmv7-linux-androideabi\nx86_64-linux-android\nwasm32-unknown-unknown\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .with(
                "wasm-bindgen --version",
                ok(&format!("wasm-bindgen {WASM_BINDGEN_PINNED}\n")),
            )
            .with("wasm-opt --version", ok("wasm-opt version 130\n"))
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
            ok("aarch64-linux-android\nwasm32-unknown-unknown\n"),
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

    /// The Web area is present on every host and green when the whole
    /// browser toolchain is installed.
    #[test]
    fn a_complete_web_toolchain_is_an_ok_area_on_any_host() {
        let runner = all_green_runner();
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let report = build_report(&ctx);
        let web = report.areas.iter().find(|a| a.name == "Web").unwrap();
        assert_eq!(web.status(), ComponentStatus::Ok);
        assert_eq!(web.components.len(), 3);
        assert_eq!(report.rollup(), ComponentStatus::Ok);
    }

    /// The area's whole point: a host with no browser toolchain at all is a
    /// `Partial` rollup, never `Missing` — web is optional everywhere, so it
    /// must not gate handback the way the core area does.
    #[test]
    fn a_fully_missing_web_area_degrades_the_rollup_to_partial_not_missing() {
        let runner = all_green_runner()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\narmv7-linux-androideabi\nx86_64-linux-android\n"),
            )
            .missing("wasm-bindgen --version")
            .missing("wasm-opt --version");
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let report = build_report(&ctx);
        let web = report.areas.iter().find(|a| a.name == "Web").unwrap();
        assert!(
            web.components
                .iter()
                .all(|c| c.status == ComponentStatus::Partial),
            "{web:?}"
        );
        assert_eq!(web.status(), ComponentStatus::Partial);
        assert_eq!(report.rollup(), ComponentStatus::Partial);
    }

    /// The fix shapes a wizard renders: runnable commands for the target and
    /// the CLI, guidance for binaryen (no single install command exists).
    #[test]
    fn web_fixes_are_runnable_for_the_target_and_cli_and_guidance_for_wasm_opt() {
        let runner = all_green_runner()
            .with(
                "rustup target list --installed",
                ok("x86_64-unknown-linux-gnu\n"),
            )
            .missing("wasm-bindgen --version")
            .missing("wasm-opt --version");
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let report = build_report(&ctx);
        let web = report.areas.iter().find(|a| a.name == "Web").unwrap();

        let target = web
            .components
            .iter()
            .find(|c| c.name == "wasm32 target")
            .unwrap();
        let fix = &target.fix_commands[0];
        assert!(fix.auto_runnable);
        assert_eq!(fix.program, "rustup");
        assert_eq!(fix.args, vec!["target", "add", "wasm32-unknown-unknown"]);

        let bindgen = web
            .components
            .iter()
            .find(|c| c.name == "wasm-bindgen CLI")
            .unwrap();
        let fix = &bindgen.fix_commands[0];
        assert!(fix.auto_runnable);
        assert_eq!(fix.program, "cargo");
        assert_eq!(
            fix.args,
            vec![
                "install",
                "-f",
                "wasm-bindgen-cli",
                "--version",
                WASM_BINDGEN_PINNED
            ]
        );

        let wasm_opt = web
            .components
            .iter()
            .find(|c| c.name == "wasm-opt")
            .unwrap();
        let fix = &wasm_opt.fix_commands[0];
        assert!(!fix.auto_runnable);
        assert!(fix.program.is_empty());
        assert_eq!(
            fix.doc_link.as_deref(),
            Some("https://github.com/WebAssembly/binaryen/releases")
        );
    }

    /// A CLI whose version differs from the framework pin is a `Partial`
    /// component carrying the install command for the pinned version.
    #[test]
    fn a_bindgen_version_mismatch_is_partial_with_the_pinned_install_command() {
        let runner =
            all_green_runner().with("wasm-bindgen --version", ok("wasm-bindgen 0.2.100\n"));
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let report = build_report(&ctx);
        let bindgen = report
            .areas
            .iter()
            .find(|a| a.name == "Web")
            .unwrap()
            .components
            .iter()
            .find(|c| c.name == "wasm-bindgen CLI")
            .unwrap();
        assert_eq!(bindgen.status, ComponentStatus::Partial);
        assert!(bindgen.summary.contains("0.2.100"), "{}", bindgen.summary);
        assert_eq!(
            bindgen.fix_commands[0].display,
            format!("cargo install -f wasm-bindgen-cli --version {WASM_BINDGEN_PINNED}")
        );
    }

    /// The Web area's fix strings are never re-derived independently of the
    /// probe modules that own them — each display string matches the
    /// corresponding module's own helper output exactly, the same guarantee
    /// [`crate::web_build::preflight`] gives for its own rows.
    #[test]
    fn web_area_fix_text_matches_the_probe_modules_own_helpers() {
        let runner = all_green_runner()
            .with(
                "rustup target list --installed",
                ok("x86_64-unknown-linux-gnu\n"),
            )
            .missing("wasm-bindgen --version")
            .missing("wasm-opt --version");
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let report = build_report(&ctx);
        let web = report.areas.iter().find(|a| a.name == "Web").unwrap();

        let target = web
            .components
            .iter()
            .find(|c| c.name == "wasm32 target")
            .unwrap();
        assert_eq!(
            target.fix_commands[0].display,
            wasm_target::add_target_command()
        );

        let bindgen = web
            .components
            .iter()
            .find(|c| c.name == "wasm-bindgen CLI")
            .unwrap();
        assert_eq!(
            bindgen.fix_commands[0].display,
            wasm_bindgen_cli::install_command(WASM_BINDGEN_PINNED)
        );

        let wasm_opt_row = web
            .components
            .iter()
            .find(|c| c.name == "wasm-opt")
            .unwrap();
        assert_eq!(
            wasm_opt_row.fix_commands[0].display,
            wasm_opt::INSTALL_GUIDANCE
        );
        assert_eq!(
            wasm_opt_row.fix_commands[0].doc_link.as_deref(),
            Some(wasm_opt::INSTALL_DOC_LINK)
        );
    }

    #[test]
    fn jdk_component_reports_env_var_as_its_resolution_source() {
        let runner = all_green_runner();
        let env = all_green_env();
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let report = build_report(&ctx);
        let android = report.areas.iter().find(|a| a.name == "Android").unwrap();
        let jdk = android.components.iter().find(|c| c.name == "JDK").unwrap();
        assert_eq!(jdk.status, ComponentStatus::Ok);
        assert!(jdk.summary.contains("JAVA_HOME"), "{}", jdk.summary);
    }

    #[test]
    fn jdk_component_reports_path_java_as_its_resolution_source_when_java_home_unset() {
        let runner = all_green_runner().with(
            "java -XshowSettings:properties -version",
            ok_stderr(
                "openjdk version \"17.0.9\" 2024-01-16\njava.home = /usr/lib/jvm/java-17-openjdk\n",
            ),
        ).with(
            "/usr/lib/jvm/java-17-openjdk/bin/java -version",
            ok_stderr("openjdk version \"17.0.9\" 2024-01-16\n"),
        );
        let env = FakeEnv::new()
            .set("ANDROID_HOME", "/sdk")
            .set("ANDROID_NDK_HOME", "/sdk/ndk/26.1.10909125"); // no JAVA_HOME
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let report = build_report(&ctx);
        let android = report.areas.iter().find(|a| a.name == "Android").unwrap();
        let jdk = android.components.iter().find(|c| c.name == "JDK").unwrap();
        assert_eq!(jdk.status, ComponentStatus::Ok);
        assert!(jdk.summary.contains("java` on PATH"), "{}", jdk.summary);
        assert!(
            jdk.summary.contains("/usr/lib/jvm/java-17-openjdk"),
            "{}",
            jdk.summary
        );
    }
}
