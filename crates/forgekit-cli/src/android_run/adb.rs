//! `adb install`/`am start`/`pidof`/`logcat` command construction and
//! execution (spec §12.4 steps 5-7).

use std::time::Duration;

use anyhow::{Result, bail};

use crate::process::{Output, ProcessRunner};

/// `adb -s <id> install -r <apk>` (spec §12.4 step 5).
pub fn install(runner: &dyn ProcessRunner, device_id: &str, apk_path: &str) -> Result<Output> {
    runner.run("adb", &["-s", device_id, "install", "-r", apk_path])
}

/// `adb -s <id> shell am start -n <appId>/.MainActivity` (spec §12.4 step 6;
/// `MainActivity` lives in the app's own package, per task 22's template).
pub fn launch(runner: &dyn ProcessRunner, device_id: &str, app_id: &str) -> Result<Output> {
    let target = format!("{app_id}/.MainActivity");
    runner.run(
        "adb",
        &["-s", device_id, "shell", "am", "start", "-n", &target],
    )
}

/// The real per-attempt delay used by [`resolve_pid`]'s retry loop.
pub const PID_RETRY_DELAY: Duration = Duration::from_millis(500);
/// The real number of `pidof` attempts before giving up.
pub const PID_RETRY_ATTEMPTS: u32 = 10;

/// Resolves the launched app's pid via `adb shell pidof` (spec §12.4 step
/// 7), retrying up to `max_attempts` times since app start isn't
/// instantaneous. `sleep` is called between attempts (not after the last)
/// — injected so tests don't block on a real clock.
pub fn resolve_pid(
    runner: &dyn ProcessRunner,
    device_id: &str,
    app_id: &str,
    max_attempts: u32,
    sleep: &mut dyn FnMut(),
) -> Result<String> {
    for attempt in 0..max_attempts.max(1) {
        let out = runner.run("adb", &["-s", device_id, "shell", "pidof", app_id])?;
        let pid = out.stdout.trim();
        if !pid.is_empty() {
            return Ok(pid.to_string());
        }
        if attempt + 1 < max_attempts {
            sleep();
        }
    }
    bail!("`{app_id}` did not report a pid within the retry window (adb shell pidof empty)");
}

/// Streams `adb -s <id> logcat --pid <pid>` to `on_line` (spec §12.4 step
/// 7) until the child exits — normally via Ctrl-C killing it, see
/// `commands::run`.
pub fn stream_logcat(
    runner: &dyn ProcessRunner,
    device_id: &str,
    pid: &str,
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    runner.run_streaming(
        "adb",
        &["-s", device_id, "logcat", "--pid", pid],
        None,
        &[],
        on_line,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::FakeProcessRunner;
    use std::cell::Cell;

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
            "adb -s emulator-5554 install -r android/app/build/outputs/apk/debug/app-debug.apk",
            ok(""),
        );
        let out = install(
            &runner,
            "emulator-5554",
            "android/app/build/outputs/apk/debug/app-debug.apk",
        )
        .unwrap();
        assert!(out.success);
    }

    #[test]
    fn launch_targets_main_activity_in_app_package() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell am start -n dev.f0x.myapp/.MainActivity",
            ok(""),
        );
        let out = launch(&runner, "emulator-5554", "dev.f0x.myapp").unwrap();
        assert!(out.success);
    }

    #[test]
    fn resolve_pid_returns_immediately_on_first_success() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell pidof dev.f0x.myapp",
            ok("4242\n"),
        );
        let mut sleeps = 0;
        let pid = resolve_pid(&runner, "emulator-5554", "dev.f0x.myapp", 5, &mut || {
            sleeps += 1
        })
        .unwrap();
        assert_eq!(pid, "4242");
        assert_eq!(sleeps, 0);
    }

    /// A runner that returns an empty `pidof` result for the first N calls,
    /// then a pid — models the app taking a beat to start.
    struct FlakyPidofRunner {
        calls_before_success: u32,
        calls: Cell<u32>,
    }

    impl ProcessRunner for FlakyPidofRunner {
        fn run(&self, cmd: &str, args: &[&str]) -> Result<Output> {
            assert_eq!(cmd, "adb");
            assert_eq!(
                args,
                ["-s", "emulator-5554", "shell", "pidof", "dev.f0x.myapp"]
            );
            let n = self.calls.get();
            self.calls.set(n + 1);
            if n < self.calls_before_success {
                Ok(ok(""))
            } else {
                Ok(ok("4242\n"))
            }
        }

        fn run_streaming(
            &self,
            _cmd: &str,
            _args: &[&str],
            _cwd: Option<&std::path::Path>,
            _env: &[(&str, &str)],
            _on_line: &mut dyn FnMut(&str),
        ) -> Result<Output> {
            unreachable!("resolve_pid never streams")
        }
    }

    #[test]
    fn resolve_pid_retries_until_pid_appears() {
        let runner = FlakyPidofRunner {
            calls_before_success: 3,
            calls: Cell::new(0),
        };
        let mut sleeps = 0;
        let pid = resolve_pid(&runner, "emulator-5554", "dev.f0x.myapp", 5, &mut || {
            sleeps += 1
        })
        .unwrap();
        assert_eq!(pid, "4242");
        assert_eq!(sleeps, 3);
    }

    #[test]
    fn resolve_pid_gives_up_after_max_attempts() {
        let runner = FlakyPidofRunner {
            calls_before_success: 100,
            calls: Cell::new(0),
        };
        let mut sleeps = 0;
        let err = resolve_pid(&runner, "emulator-5554", "dev.f0x.myapp", 3, &mut || {
            sleeps += 1
        })
        .unwrap_err();
        assert!(err.to_string().contains("did not report a pid"), "{err}");
        assert_eq!(sleeps, 2); // sleeps between attempts, not after the last
    }

    #[test]
    fn stream_logcat_targets_pid_and_device() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 logcat --pid 4242",
            ok("D/forgekit: hello\n"),
        );
        let mut lines = Vec::new();
        let out = stream_logcat(&runner, "emulator-5554", "4242", &mut |line| {
            lines.push(line.to_string())
        })
        .unwrap();
        assert!(out.success);
        assert_eq!(lines, vec!["D/forgekit: hello"]);
    }
}
