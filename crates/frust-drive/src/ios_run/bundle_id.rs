//! Launch identity read back out of the `.app` that was just built, via
//! `plutil -extract CFBundleIdentifier raw <app>/Info.plist`.
//!
//! `frust run`'s physical-device path used to launch the `frust.toml`-derived
//! bundle id. That is wrong for any build whose Xcode target rewrites
//! `PRODUCT_BUNDLE_IDENTIFIER` — the case `--flavor <f>` creates, since a
//! flavor selects scheme `<Flavor>` / configuration `<Mode>-<Flavor>` (see
//! `ios_build::schemes`), and the conventional way an Xcode project separates
//! a flavor's installs is a per-configuration bundle-id suffix. The build and
//! the install both follow the flavor, so the *installed* app carries the
//! suffixed id while `devicectl device process launch` was handed the base
//! one, and the device answers `CoreDeviceError 10002 … is not installed`.
//!
//! The built bundle already carries the answer in its `Info.plist`, so this
//! module reads it back out and hands `prepare_physical_session` the id to
//! launch. Every failure mode — no `plutil` on `PATH`, a non-zero exit (a
//! missing key, an unreadable plist), or output that isn't a valid bundle
//! identifier — yields `None` plus a one-line warning for the caller's
//! `on_line` sink, never an error: the caller then falls back to the
//! `frust.toml`-derived id, which is exactly right for the (overwhelmingly
//! common) no-flavor project. Mirrors `android_run::badging`'s
//! value-plus-warning shape, for the same reason — this core stays print-free.

use std::path::Path;

use crate::process::ProcessRunner;

/// The plist every `.app` bundle carries at its root. iOS bundles are flat
/// (no `Contents/` level, unlike a macOS bundle), so this is a direct child.
const INFO_PLIST: &str = "Info.plist";

/// The macOS plist reader this module shells out to. Ships with the base
/// system rather than with Xcode, so — unlike the Android side's `aapt2` —
/// there is no SDK path to locate and a bare `PATH` lookup is enough.
const PLUTIL: &str = "plutil";

/// Reads the bundle identifier of the `.app` at `app_path` through
/// `plutil -extract CFBundleIdentifier raw`, returning it plus an optional
/// one-line warning to surface through the caller's `on_line` sink.
///
/// Never fails: any problem resolves to `(None, Some(warning))` and the caller
/// falls back to the `frust.toml`-derived bundle id.
pub fn resolve(runner: &dyn ProcessRunner, app_path: &str) -> (Option<String>, Option<String>) {
    let plist = Path::new(app_path).join(INFO_PLIST);
    let plist = plist.to_string_lossy();
    match runner.run(PLUTIL, &["-extract", "CFBundleIdentifier", "raw", &plist]) {
        Ok(out) if out.success => match parse(&out.stdout) {
            Some(bundle_id) => (Some(bundle_id.to_string()), None),
            None => (
                None,
                Some(warning(
                    "it printed no valid `CFBundleIdentifier` value for the built bundle",
                )),
            ),
        },
        Ok(out) => (
            None,
            Some(warning(&format!(
                "`{PLUTIL} -extract CFBundleIdentifier raw {plist}` exited non-zero{}",
                first_line(&out.stderr)
            ))),
        ),
        Err(err) => (
            None,
            Some(warning(&format!("`{PLUTIL}` could not be run ({err})"))),
        ),
    }
}

/// The one-line warning shape every fallback path emits. Names the concrete
/// consequence (a flavor whose target rewrites the bundle id won't launch)
/// rather than only the tool failure, since on a no-flavor project the
/// fallback is perfectly correct and the warning is pure noise otherwise.
fn warning(reason: &str) -> String {
    format!(
        "warning: could not read the built `.app`'s bundle identifier ({reason}); falling back to \
         the `frust.toml` bundle identifier. A flavor whose Xcode configuration rewrites \
         `PRODUCT_BUNDLE_IDENTIFIER` will fail to launch with `application … is not installed`."
    )
}

/// `": <first non-empty stderr line>"`, or `""` when the tool said nothing —
/// keeps [`warning`] to a single line regardless of how chatty the failure was.
fn first_line(stderr: &str) -> String {
    match stderr.lines().map(str::trim).find(|line| !line.is_empty()) {
        Some(line) => format!(": {line}"),
        None => String::new(),
    }
}

/// Parses `plutil -extract … raw` output into a bundle identifier, or `None`
/// when it is empty or not a grammatically valid (shell-safe) identifier.
///
/// `raw` prints the bare scalar; whether it is newline-terminated has varied
/// across macOS releases, so the first non-empty line is taken rather than the
/// whole trimmed buffer.
fn parse(stdout: &str) -> Option<&str> {
    let value = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    // The value flows verbatim into `devicectl device process launch …
    // <bundle_id>`, so it is validated at this single point of resolution —
    // the same invariant `ios_run::project::detect` enforces for the
    // `frust.toml`-derived id (see `crate::ios_id`). A build output is not
    // user input, but "validate where the value is resolved" is the rule, not
    // "trust this particular producer".
    crate::ios_id::validate(value).ok()?;
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};

    const APP: &str = "/tmp/myapp/build/ios/Build/Products/Debug-Develop-iphoneos/Runner.app";

    /// Mirrors [`resolve`]'s own `Path::new(app_path).join(INFO_PLIST)` +
    /// `to_string_lossy()` exactly, rather than a hand-formatted
    /// `"{app_path}/{INFO_PLIST}"`. `APP` is a `/`-only literal, but
    /// `PathBuf::push` always inserts the *host's* separator (a backslash on
    /// Windows) regardless of the separator style already in the base
    /// string, so a hand-formatted key with a literal `/` before
    /// `Info.plist` silently drifts from the real invocation key there —
    /// building it the same way `resolve` does keeps the two identical on
    /// every host.
    fn plutil_key(app_path: &str) -> String {
        let plist = Path::new(app_path).join(INFO_PLIST);
        format!(
            "{PLUTIL} -extract CFBundleIdentifier raw {}",
            plist.display()
        )
    }

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    #[test]
    fn parse_reads_a_newline_terminated_value() {
        assert_eq!(parse("dev.f0x.myapp\n"), Some("dev.f0x.myapp"));
    }

    #[test]
    fn parse_reads_an_unterminated_value() {
        assert_eq!(parse("dev.f0x.myapp"), Some("dev.f0x.myapp"));
    }

    #[test]
    fn parse_reads_a_suffixed_flavor_value() {
        // The case this module exists for: the built target's own id, which
        // no `frust.toml` value derives.
        assert_eq!(
            parse("dev.f0x.myapp.develop\n"),
            Some("dev.f0x.myapp.develop")
        );
    }

    #[test]
    fn parse_returns_none_on_empty_output() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("\n  \n"), None);
    }

    #[test]
    fn parse_rejects_shell_metacharacters_rather_than_forwarding_them_to_devicectl() {
        assert_eq!(parse("x; rm -rf /\n"), None);
        assert_eq!(parse("$(reboot)\n"), None);
    }

    #[test]
    fn parse_rejects_a_single_segment_value() {
        // `plutil` will happily print whatever scalar the key holds; a
        // one-segment string is not a bundle identifier.
        assert_eq!(parse("Runner\n"), None);
    }

    #[test]
    fn resolve_returns_the_plist_bundle_id_and_no_warning() {
        let runner = FakeProcessRunner::new().with(plutil_key(APP), ok("dev.f0x.myapp.develop\n"));
        let (bundle_id, warning) = resolve(&runner, APP);
        assert_eq!(bundle_id.as_deref(), Some("dev.f0x.myapp.develop"));
        assert_eq!(warning, None);
    }

    #[test]
    fn resolve_reads_info_plist_inside_the_app_bundle() {
        // The fixture key is the only registered invocation, so a resolver
        // that pointed `plutil` anywhere else would fail to match and warn.
        let runner = FakeProcessRunner::new().with(plutil_key(APP), ok("dev.f0x.myapp\n"));
        let (bundle_id, _) = resolve(&runner, APP);
        assert_eq!(bundle_id.as_deref(), Some("dev.f0x.myapp"));
    }

    #[test]
    fn resolve_falls_back_with_one_warning_when_plutil_cannot_be_spawned() {
        let runner = FakeProcessRunner::new().missing(PLUTIL);
        let (bundle_id, warning) = resolve(&runner, APP);
        assert_eq!(bundle_id, None);
        let warning = warning.expect("a fallback warning");
        assert!(warning.starts_with("warning:"), "{warning}");
        assert!(warning.contains("PRODUCT_BUNDLE_IDENTIFIER"), "{warning}");
        assert_eq!(warning.lines().count(), 1, "one line only: {warning}");
    }

    #[test]
    fn resolve_falls_back_with_one_warning_on_a_non_zero_exit() {
        let runner = FakeProcessRunner::new().with(
            plutil_key(APP),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "No value at that key path or invalid key path: CFBundleIdentifier\n"
                    .to_string(),
            },
        );
        let (bundle_id, warning) = resolve(&runner, APP);
        assert_eq!(bundle_id, None);
        let warning = warning.expect("a fallback warning");
        assert!(warning.contains("invalid key path"), "{warning}");
        assert_eq!(warning.lines().count(), 1, "one line only: {warning}");
    }

    #[test]
    fn resolve_falls_back_with_one_warning_on_unusable_output() {
        let runner = FakeProcessRunner::new().with(plutil_key(APP), ok("<dict>\n"));
        let (bundle_id, warning) = resolve(&runner, APP);
        assert_eq!(bundle_id, None);
        let warning = warning.expect("a fallback warning");
        assert!(warning.contains("CFBundleIdentifier"), "{warning}");
        assert_eq!(warning.lines().count(), 1, "one line only: {warning}");
    }
}
