//! `devicectl` install/launch command construction and execution for the
//! physical-iOS-device run pipeline (task 67). Mirrors `ios_run::simctl`'s
//! shape, but `devicectl`'s argv differs from `simctl`'s (`--device` is
//! named, not positional, and launch takes `--console
//! --terminate-existing`) — see RESEARCH.md §C for the verified argv this
//! module must not drift from.

use anyhow::Result;

use crate::process::{Output, ProcessRunner};

/// `xcrun devicectl device install app --device <udid> <app_path>`.
pub fn install(runner: &dyn ProcessRunner, udid: &str, app_path: &str) -> Result<Output> {
    runner.run(
        "xcrun",
        &[
            "devicectl",
            "device",
            "install",
            "app",
            "--device",
            udid,
            app_path,
        ],
    )
}

/// `xcrun devicectl device process launch --device <udid> --console
/// --terminate-existing <bundle_id>`. Blocks and streams the app's console
/// output through `on_line` until the app exits or the command is
/// interrupted — mirrors `simctl::launch`'s streamed UX.
pub fn launch(
    runner: &dyn ProcessRunner,
    udid: &str,
    bundle_id: &str,
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    runner.run_streaming(
        "xcrun",
        &[
            "devicectl",
            "device",
            "process",
            "launch",
            "--device",
            udid,
            "--console",
            "--terminate-existing",
            bundle_id,
        ],
        None,
        &[],
        on_line,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::FakeProcessRunner;

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    #[test]
    fn install_builds_expected_command() {
        let runner = FakeProcessRunner::new().with(
            "xcrun devicectl device install app --device 00008110-000A2D3A3C68801E /tmp/myapp/build/ios/Build/Products/Debug-iphoneos/Runner.app",
            ok(""),
        );
        let out = install(
            &runner,
            "00008110-000A2D3A3C68801E",
            "/tmp/myapp/build/ios/Build/Products/Debug-iphoneos/Runner.app",
        )
        .unwrap();
        assert!(out.success);
    }

    #[test]
    fn launch_puts_console_and_terminate_existing_flags_before_bundle_id() {
        let runner = FakeProcessRunner::new().with(
            "xcrun devicectl device process launch --device 00008110-000A2D3A3C68801E --console --terminate-existing dev.f0x.myapp",
            ok("hello from app\n"),
        );
        let mut lines = Vec::new();
        let out = launch(
            &runner,
            "00008110-000A2D3A3C68801E",
            "dev.f0x.myapp",
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert!(out.success);
        assert_eq!(lines, vec!["hello from app"]);
    }
}
