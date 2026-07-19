//! Preflight checks for `frust run`'s iOS simulator path (mirrors
//! `android_run::preflight`). Each check fails fast with a single,
//! actionable message rather than accumulating a report — `frust run`
//! should stop at the first thing standing between the user and a working
//! build.

use crate::process::ProcessRunner;

pub struct PreflightCtx<'a> {
    pub runner: &'a dyn ProcessRunner,
    /// The simulator udid the pipeline is about to build/install/launch on
    /// — checked for `Booted` state here.
    pub udid: &'a str,
}

/// Runs every check in order, stopping at (and returning) the first
/// failure's actionable message.
pub fn run(ctx: &PreflightCtx) -> Result<(), String> {
    check_xcode(ctx)?;
    check_rust_target(ctx)?;
    check_booted_simulator(ctx)?;
    Ok(())
}

fn check_xcode(ctx: &PreflightCtx) -> Result<(), String> {
    match ctx.runner.run("xcode-select", &["-p"]) {
        Ok(out) if out.success => Ok(()),
        _ => Err("Xcode not found. Run: xcode-select --install".to_string()),
    }
}

fn check_rust_target(ctx: &PreflightCtx) -> Result<(), String> {
    match ctx.runner.run("rustup", &["target", "list", "--installed"]) {
        Ok(out) if out.success && has_target(&out.stdout, "aarch64-apple-ios-sim") => Ok(()),
        _ => Err(
            "missing Rust target aarch64-apple-ios-sim. Run: rustup target add aarch64-apple-ios-sim"
                .to_string(),
        ),
    }
}

fn has_target(installed: &str, target: &str) -> bool {
    installed.lines().any(|line| line.trim() == target)
}

fn check_booted_simulator(ctx: &PreflightCtx) -> Result<(), String> {
    let out = ctx
        .runner
        .run("xcrun", &["simctl", "list", "devices", "--json"])
        .map_err(|_| "xcrun simctl not found".to_string())?;
    if !out.success {
        return Err("`xcrun simctl list devices` failed".to_string());
    }
    if is_booted(&out.stdout, ctx.udid) {
        Ok(())
    } else {
        Err(format!(
            "simulator `{}` is not booted. Boot it via Simulator.app or `xcrun simctl boot {}`.",
            ctx.udid, ctx.udid
        ))
    }
}

/// Parses `xcrun simctl list devices --json`'s
/// `{"devices": {"<runtime>": [{udid, name, state}]}}` structure, checking
/// whether `udid` appears with `state == "Booted"` — a raw `serde_json::Value`
/// walk (rather than a typed struct) since this module only needs the one
/// boolean answer, not the full device listing `devices::ios_simulator`
/// already parses.
fn is_booted(json: &str, udid: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return false;
    };
    let Some(devices) = value.get("devices").and_then(|d| d.as_object()) else {
        return false;
    };
    devices.values().any(|list| {
        list.as_array().is_some_and(|entries| {
            entries.iter().any(|device| {
                device.get("udid").and_then(|v| v.as_str()) == Some(udid)
                    && device.get("state").and_then(|v| v.as_str()) == Some("Booted")
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    const BOOTED_JSON: &str = r#"{
        "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-17-5": [
                {"udid": "AAAA", "name": "iPhone 15", "state": "Booted", "isAvailable": true},
                {"udid": "BBBB", "name": "iPhone 15 Pro", "state": "Shutdown", "isAvailable": true}
            ]
        }
    }"#;

    fn full_runner() -> FakeProcessRunner {
        FakeProcessRunner::new()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios-sim\n"),
            )
            .with("xcrun simctl list devices --json", ok(BOOTED_JSON))
    }

    #[test]
    fn passes_when_everything_present_and_simulator_booted() {
        let runner = full_runner();
        let ctx = PreflightCtx {
            runner: &runner,
            udid: "AAAA",
        };
        assert!(run(&ctx).is_ok());
    }

    #[test]
    fn fails_with_actionable_message_when_xcode_missing() {
        let runner = FakeProcessRunner::new().missing("xcode-select -p");
        let ctx = PreflightCtx {
            runner: &runner,
            udid: "AAAA",
        };
        let err = run(&ctx).unwrap_err();
        assert!(err.contains("xcode-select --install"), "{err}");
    }

    #[test]
    fn fails_with_actionable_message_when_target_missing() {
        let runner = FakeProcessRunner::new()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "rustup target list --installed",
                ok("x86_64-apple-darwin\n"),
            );
        let ctx = PreflightCtx {
            runner: &runner,
            udid: "AAAA",
        };
        let err = run(&ctx).unwrap_err();
        assert!(
            err.contains("rustup target add aarch64-apple-ios-sim"),
            "{err}"
        );
    }

    #[test]
    fn fails_when_simulator_not_booted() {
        let runner = full_runner();
        let ctx = PreflightCtx {
            runner: &runner,
            udid: "BBBB",
        };
        let err = run(&ctx).unwrap_err();
        assert!(err.contains("not booted"), "{err}");
    }

    #[test]
    fn fails_when_simulator_udid_unknown() {
        let runner = full_runner();
        let ctx = PreflightCtx {
            runner: &runner,
            udid: "ZZZZ",
        };
        let err = run(&ctx).unwrap_err();
        assert!(err.contains("not booted"), "{err}");
    }

    #[test]
    fn is_booted_true_for_matching_udid_and_state() {
        assert!(is_booted(BOOTED_JSON, "AAAA"));
        assert!(!is_booted(BOOTED_JSON, "BBBB"));
        assert!(!is_booted(BOOTED_JSON, "ZZZZ"));
    }

    #[test]
    fn is_booted_false_on_invalid_json() {
        assert!(!is_booted("not json", "AAAA"));
    }
}
