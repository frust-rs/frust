//! `adb install`/`am start`/`pidof`/`logcat` command construction and
//! execution.

use std::time::Duration;

use anyhow::{Result, bail};

use crate::process::{Output, ProcessRunner};

/// `adb -s <id> install -r <apk>`.
pub fn install(runner: &dyn ProcessRunner, device_id: &str, apk_path: &str) -> Result<Output> {
    runner.run("adb", &["-s", device_id, "install", "-r", apk_path])
}

/// Fallback ABI when the device doesn't report a usable one — the same
/// `arm64-v8a` the Gradle template previously built implicitly by default,
/// made explicit.
pub const DEFAULT_ABI: &str = "arm64-v8a";

/// Queries the connected device/emulator's primary ABI via `adb -s <id>
/// shell getprop ro.product.cpu.abi` (the mode-aware Android run pipeline
/// needs the real device ABI, not an assumed `arm64-v8a`, to pass
/// `-Pfrust.targetPlatforms`) — a physical device reports `arm64-v8a` (or
/// occasionally `armeabi-v7a`), an x86_64 emulator reports `x86_64`. Falls
/// back to [`DEFAULT_ABI`] on a spawn failure, a non-zero exit, or empty
/// output rather than failing the whole run over a single `getprop` hiccup.
pub fn device_abi(runner: &dyn ProcessRunner, device_id: &str) -> String {
    match runner.run(
        "adb",
        &["-s", device_id, "shell", "getprop", "ro.product.cpu.abi"],
    ) {
        Ok(out) if out.success => {
            let abi = out.stdout.trim();
            if abi.is_empty() {
                DEFAULT_ABI.to_string()
            } else {
                abi.to_string()
            }
        }
        _ => DEFAULT_ABI.to_string(),
    }
}

/// `adb -s <id> shell am start -n <component>`, where `component` is a
/// `<package>/<activity>` pair — normally
/// [`badging::LaunchIdentity::component`](super::badging::LaunchIdentity::component)
/// read back out of the built APK, else [`default_component`]'s
/// `frust.toml`-derived fallback.
///
/// `component` is interpolated into a device-shell command line, so both of
/// its halves must already be grammar-validated by the time they reach here.
/// There are exactly two sources, each validating at the single point it
/// resolves a value (via `crate::android_id::validate`):
/// `android_run::project::detect` for `Project::app_id` (fed through
/// [`default_component`]) and `android_run::badging::parse` for a badging-read
/// package/activity. Do not add a third that skips that validation.
pub fn launch(runner: &dyn ProcessRunner, device_id: &str, component: &str) -> Result<Output> {
    runner.run(
        "adb",
        &["-s", device_id, "shell", "am", "start", "-n", component],
    )
}

/// The pre-badging launch component: `<appId>/.MainActivity`, resolving
/// `MainActivity` relatively against the installed package. Correct whenever
/// the installed application id and the activity's namespace coincide (every
/// project without a flavor `applicationIdSuffix`), and the documented
/// fallback when the APK's badging can't be read — see
/// [`badging::resolve`](super::badging::resolve).
pub fn default_component(app_id: &str) -> String {
    format!("{app_id}/.MainActivity")
}

/// The real per-attempt delay used by [`resolve_pid`]'s retry loop.
pub const PID_RETRY_DELAY: Duration = Duration::from_millis(500);
/// The real number of `pidof` attempts before giving up.
pub const PID_RETRY_ATTEMPTS: u32 = 10;

/// Resolves the launched app's pid via `adb shell pidof`, retrying up to
/// `max_attempts` times since app start isn't instantaneous. `sleep` is
/// called between attempts (not after the last) — injected so tests don't
/// block on a real clock.
///
/// Same validated-at-source invariant as [`launch`]: `package` is
/// interpolated into `adb shell pidof <package>` and must already be
/// grammar-validated by whichever of the two sources resolved it
/// (`android_run::project::detect` or `android_run::badging::parse`) before it
/// reaches here. It must also be the *installed* package — the one
/// [`launch`]'s component named — or the poll never finds the process it just
/// started.
pub fn resolve_pid(
    runner: &dyn ProcessRunner,
    device_id: &str,
    package: &str,
    max_attempts: u32,
    sleep: &mut dyn FnMut(),
) -> Result<String> {
    for attempt in 0..max_attempts.max(1) {
        let out = runner.run("adb", &["-s", device_id, "shell", "pidof", package])?;
        let pid = out.stdout.trim();
        if !pid.is_empty() {
            return Ok(pid.to_string());
        }
        if attempt + 1 < max_attempts {
            sleep();
        }
    }
    bail!("`{package}` did not report a pid within the retry window (adb shell pidof empty)");
}

/// Streams `adb -s <id> logcat --pid <pid>` to `on_line` until the child
/// exits — normally via Ctrl-C killing it, see `commands::run`.
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
    fn device_abi_reports_getprop_output() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell getprop ro.product.cpu.abi",
            ok("x86_64\n"),
        );
        assert_eq!(device_abi(&runner, "emulator-5554"), "x86_64");
    }

    #[test]
    fn device_abi_falls_back_to_default_on_spawn_failure() {
        let runner = FakeProcessRunner::new()
            .missing("adb -s emulator-5554 shell getprop ro.product.cpu.abi");
        assert_eq!(device_abi(&runner, "emulator-5554"), DEFAULT_ABI);
    }

    #[test]
    fn device_abi_falls_back_to_default_on_empty_output() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell getprop ro.product.cpu.abi",
            ok(""),
        );
        assert_eq!(device_abi(&runner, "emulator-5554"), DEFAULT_ABI);
    }

    #[test]
    fn device_abi_falls_back_to_default_on_nonzero_exit() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell getprop ro.product.cpu.abi",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error: closed".to_string(),
            },
        );
        assert_eq!(device_abi(&runner, "emulator-5554"), DEFAULT_ABI);
    }

    #[test]
    fn default_component_targets_main_activity_relative_to_the_app_package() {
        assert_eq!(
            default_component("dev.f0x.myapp"),
            "dev.f0x.myapp/.MainActivity"
        );
    }

    #[test]
    fn launch_targets_the_default_component() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell am start -n dev.f0x.myapp/.MainActivity",
            ok(""),
        );
        let out = launch(
            &runner,
            "emulator-5554",
            &default_component("dev.f0x.myapp"),
        )
        .unwrap();
        assert!(out.success);
    }

    /// A flavored build's component: neither half is derivable from the
    /// other (`applicationIdSuffix` moves the package, not the activity's
    /// namespace-rooted class), so `launch` must pass it through verbatim.
    #[test]
    fn launch_passes_a_badging_derived_component_through_verbatim() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell am start -n dev.f0x.myapp.dev/dev.f0x.myapp.MainActivity",
            ok(""),
        );
        let out = launch(
            &runner,
            "emulator-5554",
            "dev.f0x.myapp.dev/dev.f0x.myapp.MainActivity",
        )
        .unwrap();
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

        fn spawn_streaming(
            &self,
            _cmd: &str,
            _args: &[&str],
            _cwd: Option<&std::path::Path>,
            _env: &[(&str, &str)],
        ) -> Result<crate::process::StreamHandle> {
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
            ok("D/frust: hello\n"),
        );
        let mut lines = Vec::new();
        let out = stream_logcat(&runner, "emulator-5554", "4242", &mut |line| {
            lines.push(line.to_string())
        })
        .unwrap();
        assert!(out.success);
        assert_eq!(lines, vec!["D/frust: hello"]);
    }
}
