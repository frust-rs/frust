//! `simctl install`/`launch`/`terminate` command construction and execution
//! (mirrors `android_run::adb`).

use anyhow::{Context, Result};

use crate::process::{Output, ProcessRunner, StreamHandle};

/// `xcrun simctl install <udid> <app_path>`.
pub fn install(runner: &dyn ProcessRunner, udid: &str, app_path: &str) -> Result<Output> {
    runner.run("xcrun", &["simctl", "install", udid, app_path])
}

/// `xcrun simctl launch --console-pty <udid> <bundle_id>` (flag before the
/// udid — order matters). This blocks and carries the app's stdout/stderr
/// (Rust `println!`/`eprintln!` arrive here; `log stream` would not see
/// them), streaming each line through `on_line` until the app exits or the
/// command is interrupted.
pub fn launch(
    runner: &dyn ProcessRunner,
    udid: &str,
    bundle_id: &str,
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    runner.run_streaming(
        "xcrun",
        &["simctl", "launch", "--console-pty", udid, bundle_id],
        None,
        &[],
        on_line,
    )
}

/// [`launch`] spawned through the cancellable
/// [`ProcessRunner::spawn_streaming`] seam: the same argv, the app's console
/// as a killable [`StreamHandle`]. A hot run reads the app's devtools
/// discovery line from it. Killing the handle stops the `simctl` bridge,
/// not the app ([`terminate`] does that).
pub fn spawn_launch(
    runner: &dyn ProcessRunner,
    udid: &str,
    bundle_id: &str,
) -> Result<StreamHandle> {
    runner
        .spawn_streaming(
            "xcrun",
            &["simctl", "launch", "--console-pty", udid, bundle_id],
            None,
            &[],
        )
        .with_context(|| format!("spawning `simctl launch {bundle_id}`"))
}

/// Best-effort `xcrun simctl terminate <udid> <bundle_id>` — failure is
/// ignored, since the app may already be gone by the time this runs (normal
/// exit, or the simulator having been shut down).
pub fn terminate(runner: &dyn ProcessRunner, udid: &str, bundle_id: &str) {
    let _ = runner.run("xcrun", &["simctl", "terminate", udid, bundle_id]);
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
            "xcrun simctl install AAAA /tmp/myapp/build/ios/Build/Products/Debug-iphonesimulator/Runner.app",
            ok(""),
        );
        let out = install(
            &runner,
            "AAAA",
            "/tmp/myapp/build/ios/Build/Products/Debug-iphonesimulator/Runner.app",
        )
        .unwrap();
        assert!(out.success);
    }

    #[test]
    fn launch_puts_console_pty_flag_before_udid() {
        let runner = FakeProcessRunner::new().with(
            "xcrun simctl launch --console-pty AAAA dev.f0x.myapp",
            ok("hello from app\n"),
        );
        let mut lines = Vec::new();
        let out = launch(&runner, "AAAA", "dev.f0x.myapp", &mut |line| {
            lines.push(line.to_string())
        })
        .unwrap();
        assert!(out.success);
        assert_eq!(lines, vec!["hello from app"]);
    }

    #[test]
    fn spawn_launch_streams_the_console_pty_launch() {
        let runner = FakeProcessRunner::new().with_stream(
            "xcrun simctl launch --console-pty AAAA dev.f0x.myapp",
            ["dev.f0x.myapp: 4242", "hello from app"],
            true,
        );
        let mut stream = spawn_launch(&runner, "AAAA", "dev.f0x.myapp").unwrap();
        let lines: Vec<String> = stream.lines.iter().collect();
        assert_eq!(lines, vec!["dev.f0x.myapp: 4242", "hello from app"]);
        assert!(stream.wait());
    }

    #[test]
    fn terminate_ignores_failure() {
        let runner = FakeProcessRunner::new().missing("xcrun simctl terminate AAAA dev.f0x.myapp");
        // Must not panic even though the fake runner errors on this
        // invocation — terminate is best-effort.
        terminate(&runner, "AAAA", "dev.f0x.myapp");
    }

    #[test]
    fn terminate_runs_expected_command() {
        let runner =
            FakeProcessRunner::new().with("xcrun simctl terminate AAAA dev.f0x.myapp", ok(""));
        terminate(&runner, "AAAA", "dev.f0x.myapp");
    }
}
