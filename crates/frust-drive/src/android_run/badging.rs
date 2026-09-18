//! Launch identity read back out of the APK that was just built, via the SDK
//! build-tools' `aapt2 dump badging <apk>`.
//!
//! `frust run`'s Android path used to launch `<frust.toml app id>/.MainActivity`
//! and poll `pidof <frust.toml app id>`. Both halves are wrong for a Gradle
//! product flavor declaring an `applicationIdSuffix`: the installed package
//! becomes `<base><suffix>`, while AGP rewrites the manifest's relative
//! `.MainActivity` against the module's **namespace** (which the suffix does
//! not touch) before setting the merged manifest's `package` to the
//! application id — so the installed package and the launchable activity's
//! class no longer share a prefix, and neither `-n <app_id>/.MainActivity` nor
//! `pidof <app_id>` resolves.
//!
//! The built APK already carries both values, so this module reads them back
//! out of it and hands `prepare_session` a [`LaunchIdentity`]. Every failure
//! mode — no SDK env, no `aapt2` under `build-tools/`, a spawn failure, a
//! non-zero exit, or output this parser doesn't recognize — yields `None` plus
//! a one-line warning for the caller's `on_line` sink, never an error: the
//! caller then falls back to the pre-badging behavior, which is exactly right
//! for the (overwhelmingly common) no-flavor project.

use std::path::{Path, PathBuf};

use crate::doctor::EnvLookup;
use crate::process::ProcessRunner;

/// The SDK subdirectory holding versioned build-tools installs, each of which
/// ships its own `aapt2`.
const BUILD_TOOLS_DIR: &str = "build-tools";

/// The `aapt2` executable's file name inside a build-tools version directory.
const AAPT2_BIN: &str = if cfg!(windows) { "aapt2.exe" } else { "aapt2" };

/// What the launch phase runs when the SDK copy of `aapt2` can't be located —
/// a bare PATH lookup, which succeeds for the (unusual) setup that puts
/// build-tools on `PATH` without setting `ANDROID_HOME`/`ANDROID_SDK_ROOT`,
/// and otherwise fails to spawn and takes the documented fallback.
const AAPT2_ON_PATH: &str = AAPT2_BIN;

/// The installed identity of a built APK: the package `adb shell pidof`
/// polls, and the activity class that — joined to the package by
/// [`component`](Self::component) — `adb shell am start -n` targets.
///
/// Both fields are grammar-validated by [`parse`] before a value is ever
/// constructed (see `crate::android_id::validate`), so a `LaunchIdentity` is
/// safe to interpolate into a device shell command line — the same
/// validated-at-source invariant `android_run::project::detect` gives
/// `Project::app_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchIdentity {
    /// The installed application id (`applicationId` + any flavor/build-type
    /// `applicationIdSuffix`).
    pub package: String,
    /// The fully-qualified launchable activity class, rooted at the Gradle
    /// module's `namespace` rather than at [`package`](Self::package).
    pub activity: String,
}

impl LaunchIdentity {
    /// The `<package>/<activity>` component string `adb shell am start -n`
    /// takes.
    pub fn component(&self) -> String {
        format!("{}/{}", self.package, self.activity)
    }
}

/// Reads `apk_path`'s launch identity through `aapt2 dump badging`, returning
/// it plus an optional one-line warning to surface through the caller's
/// `on_line` sink (mirroring `cargo_manifest::resolve_release_features`'
/// value-plus-warning shape — this core stays print-free).
///
/// Never fails: any problem resolves to `(None, Some(warning))` and the caller
/// falls back to the `frust.toml`-derived identity.
pub fn resolve(
    runner: &dyn ProcessRunner,
    env: &dyn EnvLookup,
    apk_path: &str,
) -> (Option<LaunchIdentity>, Option<String>) {
    let aapt2 = aapt2_command(env);
    match runner.run(&aapt2, &["dump", "badging", apk_path]) {
        Ok(out) if out.success => match parse(&out.stdout) {
            Some(identity) => (Some(identity), None),
            None => (
                None,
                Some(warning(
                    "its output carried no usable `package:`/`launchable-activity:` line",
                )),
            ),
        },
        Ok(out) => (
            None,
            Some(warning(&format!(
                "`{aapt2} dump badging` exited non-zero{}",
                first_line(&out.stderr)
            ))),
        ),
        Err(err) => (
            None,
            Some(warning(&format!("`{aapt2}` could not be run ({err})"))),
        ),
    }
}

/// The one-line warning shape every fallback path emits. Names the concrete
/// consequence (a suffixed flavor won't launch) rather than only the tool
/// failure, since on a no-flavor project the fallback is perfectly correct and
/// the warning is pure noise otherwise.
fn warning(reason: &str) -> String {
    format!(
        "warning: could not read the built APK's launch identity ({reason}); falling back to the \
         `frust.toml` application id + `.MainActivity`. A Gradle flavor declaring an \
         `applicationIdSuffix` will not launch — install the Android SDK build-tools and set \
         ANDROID_HOME so `aapt2` can be found."
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

/// Parses `aapt2 dump badging` output into a [`LaunchIdentity`], or `None`
/// when either half is missing, empty, or not a grammatically valid
/// (shell-safe) Android identifier.
///
/// Reads the FIRST `package:`/`launchable-activity:` line each — a
/// multi-launcher APK lists several launchable activities, and the first is
/// the one `am start` would resolve for a bare launch.
fn parse(stdout: &str) -> Option<LaunchIdentity> {
    let mut package: Option<&str> = None;
    let mut activity: Option<&str> = None;

    for line in stdout.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("package:")
            && package.is_none()
        {
            package = attr(rest, "name");
        } else if let Some(rest) = line.strip_prefix("launchable-activity:")
            && activity.is_none()
        {
            activity = attr(rest, "name");
        }
    }

    let package = package?;
    let activity = activity?;
    // Both flow verbatim into `adb shell am start -n <pkg>/<activity>` /
    // `adb shell pidof <pkg>`, where the device shell interprets
    // metacharacters — the same injection path `android_id`'s module doc
    // describes for `[android] identifier`. An APK is a build output rather
    // than user input, but "validate at the single point of resolution" is the
    // invariant, not "trust this particular producer".
    crate::android_id::validate(package).ok()?;
    crate::android_id::validate(activity).ok()?;

    Some(LaunchIdentity {
        package: package.to_string(),
        activity: activity.to_string(),
    })
}

/// The value of `<key>='…'` in `rest`, matched only at an attribute boundary
/// (start of `rest`, or right after whitespace) so a longer attribute whose
/// name merely *ends* in `key` can't be mistaken for it — badging's `package:`
/// line carries `compileSdkVersionCodename='14'`, which contains `name='14'`
/// as a plain substring.
fn attr<'a>(rest: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}='");
    let mut from = 0usize;
    while let Some(offset) = rest[from..].find(&needle) {
        let start = from + offset;
        let value_start = start + needle.len();
        let at_boundary = start == 0
            || rest[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if at_boundary {
            let end = rest[value_start..].find('\'')?;
            return Some(&rest[value_start..value_start + end]);
        }
        from = value_start;
    }
    None
}

/// The command [`resolve`] spawns: the newest installed build-tools `aapt2`,
/// or [`AAPT2_ON_PATH`] when the SDK copy can't be located.
fn aapt2_command(env: &dyn EnvLookup) -> String {
    locate_aapt2(env)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| AAPT2_ON_PATH.to_string())
}

/// The Android SDK root, resolved exactly as `doctor::AndroidSdkValidator`
/// does it: `ANDROID_HOME` first, then `ANDROID_SDK_ROOT`.
fn sdk_root(env: &dyn EnvLookup) -> Option<PathBuf> {
    env.get("ANDROID_HOME")
        .or_else(|| env.get("ANDROID_SDK_ROOT"))
        .map(|home| PathBuf::from(home.trim()))
        .filter(|path| !path.as_os_str().is_empty())
}

/// The `aapt2` of the newest build-tools version installed under the SDK root
/// that actually ships one — a version directory missing the binary (a partial
/// or in-progress `sdkmanager` install) is skipped rather than selected and
/// then failing to spawn.
fn locate_aapt2(env: &dyn EnvLookup) -> Option<PathBuf> {
    let build_tools = sdk_root(env)?.join(BUILD_TOOLS_DIR);
    let mut candidates: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir(&build_tools)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| aapt2_in(&entry.path()))
        .collect();
    candidates.sort();
    candidates.pop().map(|(_, aapt2)| aapt2)
}

/// `dir`'s version sort key paired with its `aapt2` path, or `None` if `dir`
/// isn't a build-tools version directory holding one.
fn aapt2_in(dir: &Path) -> Option<(Vec<u64>, PathBuf)> {
    let aapt2 = dir.join(AAPT2_BIN);
    if !aapt2.is_file() {
        return None;
    }
    let name = dir.file_name()?.to_str()?;
    Some((version_key(name), aapt2))
}

/// A numeric-component sort key for a build-tools version directory name
/// (`35.0.1`, `34.0.0-rc3`), so `35.0.1` sorts above `9.0.0` — plain
/// lexicographic string ordering gets that pair backwards. Non-numeric
/// trailing text in a component is ignored; a fully non-numeric component
/// sorts as `0`.
fn version_key(name: &str) -> Vec<u64> {
    name.split('.')
        .map(|segment| {
            segment
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::FakeEnv;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A realistic, trimmed `aapt2 dump badging` capture for a no-flavor
    /// build: package and activity share the module namespace.
    const BADGING_BASE: &str = "\
package: name='com.example.app' versionCode='1' versionName='1.0' compileSdkVersion='36' compileSdkVersionCodename='16'
sdkVersion:'24'
targetSdkVersion:'36'
application-label:'Example'
application: label='Example' icon='res/mipmap-mdpi-v4/ic_launcher.png'
launchable-activity: name='com.example.app.MainActivity'  label='Example' icon=''
feature-group: label=''
  uses-feature: name='android.hardware.faketouch'
supports-screens: 'small' 'normal' 'large' 'xlarge'
";

    /// The flavor case this whole module exists for: `applicationIdSuffix
    /// ".dev"` moves the package but not the activity's namespace-rooted
    /// class.
    const BADGING_SUFFIXED: &str = "\
package: name='com.example.app.dev' versionCode='1' versionName='1.0' compileSdkVersion='36'
sdkVersion:'24'
application-label:'Example dev'
launchable-activity: name='com.example.app.MainActivity'  label='Example dev' icon=''
";

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-badging-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Plants an SDK root with an `aapt2` under each named build-tools
    /// version directory, returning the SDK root.
    fn plant_sdk(dir: &Path, versions: &[&str]) -> PathBuf {
        let sdk = dir.join("sdk");
        for version in versions {
            let version_dir = sdk.join(BUILD_TOOLS_DIR).join(version);
            fs::create_dir_all(&version_dir).unwrap();
            fs::write(version_dir.join(AAPT2_BIN), b"#!/bin/sh\n").unwrap();
        }
        sdk
    }

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    #[test]
    fn parse_reads_package_and_launchable_activity() {
        let identity = parse(BADGING_BASE).expect("well-formed badging output");
        assert_eq!(identity.package, "com.example.app");
        assert_eq!(identity.activity, "com.example.app.MainActivity");
        assert_eq!(
            identity.component(),
            "com.example.app/com.example.app.MainActivity"
        );
    }

    #[test]
    fn parse_reads_suffixed_package_with_namespace_rooted_activity() {
        let identity = parse(BADGING_SUFFIXED).expect("well-formed badging output");
        assert_eq!(identity.package, "com.example.app.dev");
        assert_eq!(identity.activity, "com.example.app.MainActivity");
        // The whole point: neither half of the component is derivable from
        // the other, nor from `frust.toml`.
        assert_eq!(
            identity.component(),
            "com.example.app.dev/com.example.app.MainActivity"
        );
    }

    #[test]
    fn parse_takes_the_first_launchable_activity() {
        let stdout = "\
package: name='com.example.app'
launchable-activity: name='com.example.app.MainActivity'  label='' icon=''
launchable-activity: name='com.example.app.SecondActivity'  label='' icon=''
";
        let identity = parse(stdout).unwrap();
        assert_eq!(identity.activity, "com.example.app.MainActivity");
    }

    #[test]
    fn parse_ignores_a_longer_attribute_ending_in_name() {
        // `compileSdkVersionCodename='16'` contains `name='16'` as a plain
        // substring; only an attribute-boundary match may be read.
        let stdout = "\
package: compileSdkVersionCodename='16' name='com.example.app'
launchable-activity: name='com.example.app.MainActivity'
";
        let identity = parse(stdout).unwrap();
        assert_eq!(identity.package, "com.example.app");
    }

    #[test]
    fn parse_returns_none_on_empty_output() {
        assert_eq!(parse(""), None);
    }

    #[test]
    fn parse_returns_none_on_unrecognized_output() {
        assert_eq!(
            parse("ERROR: dump failed because no AndroidManifest.xml\n"),
            None
        );
    }

    #[test]
    fn parse_returns_none_without_a_launchable_activity() {
        // A library/test APK has a package but nothing to launch.
        assert_eq!(
            parse("package: name='com.example.app' versionCode='1'\n"),
            None
        );
    }

    #[test]
    fn parse_returns_none_without_a_package() {
        assert_eq!(
            parse("launchable-activity: name='com.example.app.MainActivity'\n"),
            None
        );
    }

    #[test]
    fn parse_returns_none_on_an_empty_name_value() {
        assert_eq!(
            parse("package: name=''\nlaunchable-activity: name='com.example.app.MainActivity'\n"),
            None
        );
    }

    #[test]
    fn parse_rejects_shell_metacharacters_rather_than_forwarding_them_to_adb() {
        let stdout = "\
package: name='x; rm -rf /'
launchable-activity: name='com.example.app.MainActivity'
";
        assert_eq!(parse(stdout), None);
    }

    #[test]
    fn parse_rejects_a_hostile_activity_class() {
        let stdout = "\
package: name='com.example.app'
launchable-activity: name='$(reboot)'
";
        assert_eq!(parse(stdout), None);
    }

    #[test]
    fn version_key_orders_numerically_not_lexicographically() {
        assert!(version_key("35.0.1") > version_key("9.0.0"));
        assert!(version_key("35.0.1") > version_key("35.0.0"));
        assert!(version_key("34.0.0-rc3") > version_key("33.0.2"));
    }

    #[test]
    fn locate_aapt2_prefers_the_newest_build_tools_version() {
        let dir = unique_temp_dir("newest");
        let sdk = plant_sdk(&dir, &["9.0.0", "34.0.0", "35.0.1"]);
        let env = FakeEnv::new().set("ANDROID_HOME", &sdk.to_string_lossy());
        assert_eq!(
            locate_aapt2(&env),
            Some(sdk.join(BUILD_TOOLS_DIR).join("35.0.1").join(AAPT2_BIN))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn locate_aapt2_skips_a_version_directory_without_the_binary() {
        let dir = unique_temp_dir("partial");
        let sdk = plant_sdk(&dir, &["34.0.0"]);
        // A partial `sdkmanager` install: the newest directory exists but
        // holds no `aapt2`.
        fs::create_dir_all(sdk.join(BUILD_TOOLS_DIR).join("36.0.0")).unwrap();
        assert_eq!(
            locate_aapt2(&env_for(&sdk)),
            Some(sdk.join(BUILD_TOOLS_DIR).join("34.0.0").join(AAPT2_BIN))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn locate_aapt2_falls_back_to_android_sdk_root() {
        let dir = unique_temp_dir("sdk-root");
        let sdk = plant_sdk(&dir, &["35.0.0"]);
        let env = FakeEnv::new().set("ANDROID_SDK_ROOT", &sdk.to_string_lossy());
        assert!(locate_aapt2(&env).is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn aapt2_command_falls_back_to_a_path_lookup_without_sdk_env() {
        assert_eq!(aapt2_command(&FakeEnv::new()), AAPT2_ON_PATH);
    }

    fn env_for(sdk: &Path) -> FakeEnv {
        FakeEnv::new().set("ANDROID_HOME", &sdk.to_string_lossy())
    }

    #[test]
    fn resolve_returns_the_badging_identity_and_no_warning() {
        let dir = unique_temp_dir("resolve-ok");
        let sdk = plant_sdk(&dir, &["35.0.1"]);
        let aapt2 = sdk.join(BUILD_TOOLS_DIR).join("35.0.1").join(AAPT2_BIN);
        let apk = "/tmp/app/build/android/app/outputs/apk/dev/debug/app-dev-debug.apk";
        let runner = FakeProcessRunner::new().with(
            format!("{} dump badging {apk}", aapt2.to_string_lossy()),
            ok(BADGING_SUFFIXED),
        );

        let (identity, warning) = resolve(&runner, &env_for(&sdk), apk);
        assert_eq!(warning, None);
        assert_eq!(
            identity.map(|id| id.component()),
            Some("com.example.app.dev/com.example.app.MainActivity".to_string())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_falls_back_with_one_warning_when_aapt2_cannot_be_spawned() {
        // No SDK env at all: the PATH lookup is attempted and the (empty)
        // fake runner reports the binary missing.
        let runner = FakeProcessRunner::new().missing("aapt2 dump badging");
        let (identity, warning) = resolve(&runner, &FakeEnv::new(), "/tmp/app.apk");
        assert_eq!(identity, None);
        let warning = warning.expect("a fallback warning");
        assert!(warning.starts_with("warning:"), "{warning}");
        assert!(warning.contains("applicationIdSuffix"), "{warning}");
        assert_eq!(warning.lines().count(), 1, "one line only: {warning}");
    }

    #[test]
    fn resolve_falls_back_with_one_warning_on_a_non_zero_exit() {
        let dir = unique_temp_dir("resolve-nonzero");
        let sdk = plant_sdk(&dir, &["35.0.1"]);
        let aapt2 = sdk.join(BUILD_TOOLS_DIR).join("35.0.1").join(AAPT2_BIN);
        let apk = "/tmp/app.apk";
        let runner = FakeProcessRunner::new().with(
            format!("{} dump badging {apk}", aapt2.to_string_lossy()),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "ERROR: dump failed because no AndroidManifest.xml found\n".to_string(),
            },
        );

        let (identity, warning) = resolve(&runner, &env_for(&sdk), apk);
        assert_eq!(identity, None);
        let warning = warning.expect("a fallback warning");
        assert!(warning.contains("no AndroidManifest.xml"), "{warning}");
        assert_eq!(warning.lines().count(), 1, "one line only: {warning}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_falls_back_with_one_warning_on_unparseable_output() {
        let dir = unique_temp_dir("resolve-garbage");
        let sdk = plant_sdk(&dir, &["35.0.1"]);
        let aapt2 = sdk.join(BUILD_TOOLS_DIR).join("35.0.1").join(AAPT2_BIN);
        let apk = "/tmp/app.apk";
        let runner = FakeProcessRunner::new().with(
            format!("{} dump badging {apk}", aapt2.to_string_lossy()),
            ok("something entirely unexpected\n"),
        );

        let (identity, warning) = resolve(&runner, &env_for(&sdk), apk);
        assert_eq!(identity, None);
        let warning = warning.expect("a fallback warning");
        assert!(warning.contains("launchable-activity"), "{warning}");
        let _ = fs::remove_dir_all(&dir);
    }
}
