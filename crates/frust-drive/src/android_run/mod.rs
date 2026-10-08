//! Android drive pipeline for `frust run`: preflight → gradle build → adb
//! install → launch (against the built APK's own badging-read identity, see
//! [`badging`]) → pid-scoped logcat streaming.
//! Device selection (this module's top level) is decoupled from clap/stdin
//! so it's unit-testable without a terminal.

pub mod adb;
pub mod badging;
pub mod gradle;
pub mod preflight;
pub mod project;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};

use anyhow::{Context, Result, bail};

use crate::android_build::AndroidArtifact;
use crate::build_info::{BuildInfo, BuildMode};
use crate::devices::{Device, Platform};
use crate::doctor::{EnvLookup, RealEnv};
use crate::process::{ProcessRunner, StreamHandle, tail_lines};

/// Outcome of matching discovered devices against `-d`/no-flag selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSelection {
    /// A single device to run on: either `-d` matched exactly one Android
    /// device, or exactly one Android device was discovered with no `-d`.
    Auto(Device),
    /// No Android device connected and no `-d` given: desktop fallback
    /// (`cargo run` passthrough).
    Desktop,
    /// Multiple Android devices and no `-d`: caller prompts (TTY) or
    /// lists-and-exits (non-TTY).
    Ambiguous(Vec<Device>),
    /// `-d` matched zero/multiple devices, or matched a non-Android device.
    Error(String),
}

/// Resolves device selection from all discovered devices and an optional
/// `-d` pattern (prefix match on id or name, case-insensitive).
pub fn select_device(devices: &[Device], pattern: Option<&str>) -> DeviceSelection {
    if let Some(pattern) = pattern {
        let matches: Vec<&Device> = devices
            .iter()
            .filter(|d| matches_pattern(d, pattern))
            .collect();
        return match matches.as_slice() {
            [] => DeviceSelection::Error(format!("no device matching `{pattern}`")),
            // Platform/kind-specific handling (Android drive pipeline, iOS
            // simulator drive pipeline, or a sentinel for physical iOS
            // devices) is the caller's job — `commands::run::run_on_device`
            // dispatches on `Device::platform`/`Device::kind`.
            [device] => DeviceSelection::Auto((*device).clone()),
            many => DeviceSelection::Error(format!(
                "ambiguous device id `{pattern}`; matches: {}",
                many.iter()
                    .map(|d| d.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }

    let android: Vec<Device> = devices
        .iter()
        .filter(|d| d.platform == Platform::Android)
        .cloned()
        .collect();
    match android.len() {
        0 => DeviceSelection::Desktop,
        1 => DeviceSelection::Auto(android.into_iter().next().expect("len checked")),
        _ => DeviceSelection::Ambiguous(android),
    }
}

fn matches_pattern(device: &Device, pattern: &str) -> bool {
    let pattern = pattern.to_lowercase();
    device.id.to_lowercase().starts_with(&pattern)
        || device.name.to_lowercase().starts_with(&pattern)
}

/// Parses a 1-based numbered-prompt answer (e.g. `"2"`) against `count`
/// choices, returning a 0-based index. Pure so the prompt's parsing is
/// unit-testable without stdin.
pub fn parse_prompt_selection(input: &str, count: usize) -> Result<usize, String> {
    let trimmed = input.trim();
    let choice: usize = trimmed
        .parse()
        .map_err(|_| format!("`{trimmed}` is not a number"))?;
    if choice == 0 || choice > count {
        return Err(format!("enter a number between 1 and {count}"));
    }
    Ok(choice - 1)
}

/// Whether stdout is attached to a terminal — decides numbered-prompt vs
/// list-and-exit for [`DeviceSelection::Ambiguous`].
pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// The user's home directory — [`crate::host_path::home_dir_from`]'s shared
/// `HOME`/`USERPROFILE`/`HOMEDRIVE`+`HOMEPATH` chain, fed by this crate's own
/// [`EnvLookup`] seam. The same resolver backs `frust-tui`'s
/// `engine::persist::home_dir` and `frust-dap`'s `ide_config::vscode::home_dir`,
/// so the three can no longer drift apart the way they had — this crate's own
/// chain previously stopped at `USERPROFILE`, so `HOMEDRIVE`+`HOMEPATH` is a
/// new fallback leg here (still only reachable on a Windows host with neither
/// `HOME` nor `USERPROFILE` set). Used only to print a one-time "first run
/// downloads Gradle" note ([`prepare_session`]'s `GRADLE_USER_HOME` guess),
/// never to gate the build — a `None` here just skips the note.
fn home_dir(env: &dyn EnvLookup) -> Option<PathBuf> {
    crate::host_path::home_dir_from(|key| env.get(key))
}

/// Drives the full, mode/flavor-aware Android pipeline: preflight →
/// local.properties version write → (release only) signing gate + generated
/// `.frust-signing.properties` →
/// variant-aware `./gradlew assemble<Flavor><Mode>` → variant-aware APK
/// install → badging-derived launch → pid-scoped logcat streaming.
/// Mirrors `ios_run::run`'s `(runner, root, device)` shape, plus `info` for
/// the mode/flavor/defines/version funnel and `extra_features` for the
/// front-end's `--features` passthrough (appended to the mode's own cargo
/// features; an empty slice reproduces the pre-passthrough invocation).
pub fn run(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    extra_features: &[String],
) -> Result<u8> {
    run_with_env(runner, root, device, info, extra_features, &RealEnv)
}

/// The testable core of [`run`], taking an injected [`EnvLookup`] so
/// `JAVA_HOME` resolution can be exercised with a `crate::doctor::FakeEnv`
/// in tests instead of the real process environment.
fn run_with_env(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    extra_features: &[String],
    env: &dyn EnvLookup,
) -> Result<u8> {
    // The CLI path never cancels — the shared build/install/launch core is
    // driven to completion, then `run` blocks on logcat under a Ctrl-C
    // handler (the TUI seam [`spawn_session`] takes the same core but returns
    // a killable logcat handle instead, honoring a cancel flag).
    let never = AtomicBool::new(false);
    let mut on_line = |line: &str| println!("{line}");
    let prepared = match prepare_session(
        runner,
        root,
        device,
        info,
        extra_features,
        env,
        &mut on_line,
        &never,
    )? {
        Some(prepared) => prepared,
        // Unreachable in the CLI path (`never` never sets), but keeps the
        // function total against the cancellable core.
        None => return Ok(0),
    };
    let pid = prepared.pid;

    println!("Streaming logs (pid {pid}); press Ctrl-C to stop.");
    // Default SIGINT disposition would exit 130; a streamed run wants
    // Ctrl-C to stop the (already-SIGINT'd, same-process-group) `adb
    // logcat` child and exit 0.
    //
    // This asks `crate::interrupt` — the process's single SIGINT/SIGTERM/SIGHUP
    // owner — for that exit status rather than installing a second `ctrlc`
    // handler, which would either fail to install or replace the one that
    // deletes `.frust-signing.properties`: a `--release` run has already armed
    // that scrub in `prepare_session` above.
    crate::interrupt::exit_code_on_signal(0).context("failed to install Ctrl-C handler")?;

    let mut on_log_line = |line: &str| println!("{line}");
    adb::stream_logcat(runner, &device.id, &pid, &mut on_log_line)?;

    Ok(0)
}

/// What the build → install → launch core resolves before the logcat
/// streaming phase begins: the launched app's pid, for the `logcat --pid`
/// stream both front-ends attach, plus the *installed* package it belongs to
/// (badging-resolved, so a flavor's `applicationIdSuffix` is already folded
/// in) for a caller that has to name the running app afterwards.
struct PreparedSession {
    pid: String,
    package: String,
}

/// The shared build → install → launch → resolve-pid core of the Android run
/// pipeline, feeding every phase line to `on_line` (no `println!`) and
/// checking `cancel` at each phase boundary so a stop request abandons the
/// pipeline promptly at the next boundary. Returns `Ok(None)` when `cancel`
/// was observed at a boundary (the caller reports it as a cancelled session),
/// or `Ok(Some(PreparedSession))` once the app is launched and its pid
/// resolved. A blocking phase (`./gradlew`, `adb install`) can't itself be
/// interrupted — a cancel requested mid-Gradle takes effect the moment that
/// phase returns, matching the supervisor's documented kill boundary
/// (`docs/ARCHITECTURE.md` / `supervise` module doc).
#[allow(clippy::too_many_arguments)] // the passthrough is one more caller-supplied input, not a new dependency
fn prepare_session(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    extra_features: &[String],
    env: &dyn EnvLookup,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Option<PreparedSession>> {
    prepare_session_with(
        runner,
        root,
        device,
        info,
        extra_features,
        env,
        on_line,
        cancel,
        NativeLib::Gradle,
    )
}

/// Builds and stages a hot session's native library ahead of Gradle:
/// called with the device's ABI and the session's line sink once preflight
/// has passed and before `./gradlew` runs. The hot-patch builder
/// (`hotpatch::android`) fat-links the app's `cdylib` and writes it into
/// `build/android/jniLibs/<abi>/`, where the APK packages it from.
pub type StageNativeLib<'a> = dyn FnMut(&str, &mut dyn FnMut(&str)) -> Result<()> + 'a;

/// How the app's native library reaches the APK.
enum NativeLib<'a, 'b> {
    /// The template's `cargoNdkBuild` task builds it inside Gradle: every
    /// run but a hot session's.
    Gradle,
    /// A hot session stages it first; Gradle then assembles with `-x
    /// cargoNdkBuild`, so the staged fat library is what gets packaged.
    Staged(&'a mut StageNativeLib<'b>),
}

/// [`prepare_session`] with the native library supplied by `native`.
#[allow(clippy::too_many_arguments)] // `prepare_session`'s inputs plus the native-library source
fn prepare_session_with(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    extra_features: &[String],
    env: &dyn EnvLookup,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
    native: NativeLib<'_, '_>,
) -> Result<Option<PreparedSession>> {
    let project = project::detect(root)?;
    let android_dir = project::require_android_dir(&project.root)?;

    // r1-04 replaced AGP's default jniLibs source set with the `build/`
    // redirect (setSrcDirs) in the generated template and every in-repo
    // example, so a leftover `android/app/src/main/jniLibs` is no longer
    // packaged into anything Gradle produces — flag it once, the same as
    // `android_build::build_with_env` does for `frust build`.
    crate::android_build::artifacts::warn_if_legacy_jni_libs(&project.root, on_line);

    let preflight_ctx = preflight::PreflightCtx {
        runner,
        env,
        is_macos: cfg!(target_os = "macos"),
    };
    let outcome = preflight::run(&preflight_ctx).map_err(|err| anyhow::anyhow!(err))?;

    if let Some(gradle_user_home) = home_dir(env) {
        let gradle_user_home = gradle_user_home.join(".gradle");
        if !gradle::wrapper_dist_cached(&gradle_user_home) {
            on_line("Note: first Gradle run downloads the wrapper distribution (~1-2 min).");
        }
    }

    let version_name = info.build_name.clone().unwrap_or_else(|| "1.0".to_string());
    let version_code = info
        .build_number
        .map(|n| n.to_string())
        .unwrap_or_else(|| "1".to_string());
    crate::android_build::local_properties::write(&android_dir, &version_name, &version_code)
        .context("writing android/local.properties")?;

    // One source of truth for signing (see `android_build::signing`): the gate
    // resolves `[signing]` and the material it verified is handed to Gradle as
    // `android/.frust-signing.properties`. The guard's `Drop` deletes that
    // file when this function returns — every early `?`/`bail!` included — and
    // it is dropped explicitly right after `gradle::assemble` on the happy
    // path, so the plaintext passwords never outlive the Gradle invocation.
    // The `crate::interrupt` registration it carries covers what `Drop` cannot:
    // a Ctrl-C or SIGTERM during that invocation, and an abort.
    let generated =
        crate::android_build::signing::check_release_signing(&project.root, info.mode, on_line)?
            .map(|resolved| {
                crate::android_build::signing::write_resolved(&android_dir, &resolved, on_line)
            })
            .transpose()?;
    // `Some` iff this is a release run the gate actually vouched for — a
    // non-release mode and `[signing] external = true` both resolve to `None`.
    // Recorded before the guard is dropped below, since that is exactly the
    // condition under which Gradle's own verdict has to be checked.
    let signing_promised = generated.is_some();

    if cancel.load(Ordering::SeqCst) {
        return Ok(None);
    }

    let abi = adb::device_abi(runner, &device.id);
    let target = AndroidArtifact::Apk {
        split_per_abi: false,
        abis: vec![abi.clone()],
    };
    // Release-lean preflight: drop an undeclared `lean` for a legacy app,
    // warning once through this session's `on_line` sink, so `cargo ndk`
    // never sees `--features lean` it can't resolve.
    let (features, warning) =
        crate::cargo_manifest::resolve_release_features(&project.root, info.mode, extra_features);
    if let Some(warning) = warning {
        on_line(&warning);
    }

    let task = crate::android_build::tasks::task_name(&target, info.mode, info.flavor.as_deref());
    let feature_refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let props =
        crate::android_build::tasks::gradle_properties(&target, &info.defines, &feature_refs);

    on_line(&format!("Building `{}`…", project.app_id));
    let build_start = Instant::now();
    // Wall-clock twin of `build_start`, captured at the same point: `Instant`
    // is monotonic-only (right for the elapsed-time report below) and can't
    // be compared against a file's mtime, which the freshness gate on the
    // legacy-fallback artifact (below) needs.
    let build_start_time = SystemTime::now();
    let excluded: &[&str] = match native {
        NativeLib::Gradle => &[],
        NativeLib::Staged(stage) => {
            // A hot session is a Debug build by charter; anything else
            // would package a library the variant's own cargo-ndk task
            // should have built.
            if info.mode != BuildMode::Debug {
                bail!(
                    "a hot session stages a debug library, not a {:?} one",
                    info.mode
                );
            }
            stage(&abi, on_line)?;
            if cancel.load(Ordering::SeqCst) {
                return Ok(None);
            }
            &[gradle::CARGO_NDK_BUILD_TASK]
        }
    };
    let build_out = gradle::assemble_excluding(
        runner,
        &project.root,
        &android_dir,
        &outcome.java_home,
        &task,
        excluded,
        &props,
        on_line,
    )?;
    // Gradle has returned; the generated signing file has no further reader.
    drop(generated);
    if !build_out.success {
        let tail = tail_lines(&build_out.stderr, 50);
        if tail.is_empty() {
            bail!("`./gradlew {task}` failed");
        }
        bail!("`./gradlew {task}` failed:\n{tail}");
    }
    // The gate promised a release-signed APK; Gradle just said it produced a
    // debug-signed one. Refuse before it reaches a device — the same backstop
    // `android_build::build_with_env` applies, keyed off what Gradle *did*
    // (see `android_build::signing`'s module doc).
    if signing_promised
        && crate::android_build::signing::reported_debug_signing(&format!(
            "{}\n{}",
            build_out.stdout, build_out.stderr
        ))
    {
        return Err(crate::android_build::signing::debug_signed_error());
    }
    on_line(&format!(
        "Build finished in {:.1}s.",
        build_start.elapsed().as_secs_f32()
    ));

    if cancel.load(Ordering::SeqCst) {
        return Ok(None);
    }

    // The freshness-aware counterpart of `gradle::apk_output_path` (same
    // directory-resolution/glob/count logic, via `discover_since`): an APK
    // this pipeline is about to install onto a device must not be a stale
    // leftover under the pre-migration legacy path that the project's
    // (unmigrated) Gradle config failed to actually rebuild. The
    // migrated-path fast path is unaffected — see
    // `artifacts::resolve_output_dir`'s doc comment.
    let apk_path = crate::android_build::artifacts::discover_single_apk_since(
        &project.root,
        info.mode,
        info.flavor.as_deref(),
        Some(build_start_time),
        on_line,
    )?;
    let apk_path = apk_path.to_string_lossy().into_owned();

    on_line(&format!("Installing on {}…", device.name));
    let install_start = Instant::now();
    let install_out = adb::install(runner, &device.id, &apk_path)?;
    if !install_out.success {
        bail!("`adb install` failed: {}", install_out.stderr.trim());
    }
    on_line(&format!(
        "Installed in {:.1}s.",
        install_start.elapsed().as_secs_f32()
    ));

    if cancel.load(Ordering::SeqCst) {
        return Ok(None);
    }

    // The *installed* identity, read back out of the APK just built and
    // installed rather than derived from `frust.toml`: a Gradle flavor's
    // `applicationIdSuffix` moves the package, and AGP roots the launchable
    // activity's class at the module namespace instead, so both halves of the
    // `am start -n` component (and the `pidof` package) differ from the
    // frust.toml-derived id. Unreadable badging is a warning, not an error —
    // the fallback below is exactly right for a project with no flavor.
    let (identity, badging_warning) = badging::resolve(runner, env, &apk_path);
    if let Some(badging_warning) = badging_warning {
        on_line(&badging_warning);
    }
    let (component, package) = match identity {
        Some(identity) => (identity.component(), identity.package),
        None => (
            adb::default_component(&project.app_id),
            project.app_id.clone(),
        ),
    };

    on_line(&format!("Launching {package}…"));
    let launch_out = adb::launch(runner, &device.id, &component)?;
    if !launch_out.success {
        bail!("`adb shell am start` failed: {}", launch_out.stderr.trim());
    }

    let mut sleep = || std::thread::sleep(adb::PID_RETRY_DELAY);
    let pid = adb::resolve_pid(
        runner,
        &device.id,
        &package,
        adb::PID_RETRY_ATTEMPTS,
        &mut sleep,
    )?;

    Ok(Some(PreparedSession { pid, package }))
}

/// What a [`spawn_session`] call hands back once the logcat stream is up: the
/// killable/drainable [`StreamHandle`] plus the *installed* package that was
/// actually launched, so a caller (e.g. a teardown path wanting
/// `adb shell am force-stop <package>`) doesn't have to re-derive it out of
/// the `Launching <package>…` log line. The iOS-side counterpart is
/// [`crate::ios_run::IosLaunch`].
pub struct AndroidLaunch {
    pub stream: StreamHandle,
    pub package: String,
}

/// The streaming, cancellable variant of [`run`] for a front-end that
/// supervises the session itself (the `frust-tui` supervisor). Drives the
/// same build → install → launch core, feeding each phase line to `on_line`
/// and honoring `cancel` at every phase boundary, then — rather than blocking
/// on logcat under a `ctrlc` handler the way [`run`] does — spawns the
/// `adb logcat --pid <pid>` stream through the cancellable
/// [`ProcessRunner::spawn_streaming`] seam and hands the caller its
/// [`StreamHandle`] to drain and [`kill`](StreamHandle::kill).
///
/// Returns `Ok(None)` when `cancel` was observed before the streaming phase
/// began (a stop during build/install/launch — the caller reports the session
/// as killed), or `Ok(Some(launch))` with the live logcat stream (and the
/// launched package, see [`AndroidLaunch`]) otherwise.
///
/// Killing the returned handle stops the `logcat` view of the app but does not
/// itself terminate the app on the device — a caller that wants that asks the
/// OS with `am force-stop <package>`.
pub fn spawn_session(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Option<AndroidLaunch>> {
    spawn_session_with_env(runner, root, device, info, on_line, cancel, &RealEnv)
}

/// The testable core of [`spawn_session`], taking an injected [`EnvLookup`]
/// so `JAVA_HOME` resolution can be exercised with a `crate::doctor::FakeEnv`.
fn spawn_session_with_env(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
    env: &dyn EnvLookup,
) -> Result<Option<AndroidLaunch>> {
    spawn_session_inner(
        runner,
        root,
        device,
        info,
        on_line,
        cancel,
        env,
        NativeLib::Gradle,
    )
}

/// [`spawn_session`] for a hot-patch session: the same build → install →
/// launch → logcat pipeline, except that `stage` builds and stages the
/// native library (see [`StageNativeLib`]) after preflight and Gradle then
/// runs `assemble<Flavor>Debug -x cargoNdkBuild`, so the APK packages the
/// staged library instead of rebuilding one. A failure inside `stage` ends
/// the pipeline before Gradle with that error. A non-hot run never reaches
/// this path: [`run`] and [`spawn_session`] invoke Gradle exactly as before.
pub fn spawn_hot_session(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
    stage: &mut StageNativeLib<'_>,
) -> Result<Option<AndroidLaunch>> {
    spawn_hot_session_with_env(runner, root, device, info, on_line, cancel, stage, &RealEnv)
}

/// The testable core of [`spawn_hot_session`] (see
/// [`spawn_session_with_env`]).
#[allow(clippy::too_many_arguments)] // `spawn_session_with_env`'s inputs plus the stage hook
pub(crate) fn spawn_hot_session_with_env(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
    stage: &mut StageNativeLib<'_>,
    env: &dyn EnvLookup,
) -> Result<Option<AndroidLaunch>> {
    spawn_session_inner(
        runner,
        root,
        device,
        info,
        on_line,
        cancel,
        env,
        NativeLib::Staged(stage),
    )
}

#[allow(clippy::too_many_arguments)] // the shared core of the two spawn seams
fn spawn_session_inner(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
    env: &dyn EnvLookup,
    native: NativeLib<'_, '_>,
) -> Result<Option<AndroidLaunch>> {
    // The supervisor seam exposes no `--features` flag surface of its own, so
    // the passthrough is empty here — a TUI/MCP-driven session builds exactly
    // what the mode selects, as it did before the passthrough existed.
    let Some(prepared) = prepare_session_with(
        runner,
        root,
        device,
        info,
        &[],
        env,
        on_line,
        cancel,
        native,
    )?
    else {
        return Ok(None);
    };
    if cancel.load(Ordering::SeqCst) {
        return Ok(None);
    }
    let pid = prepared.pid;
    // The `Streaming logs (pid …)` marker the supervisor's `infer_state`
    // reads to advance a session to `Running`, mirroring `run`'s own line.
    on_line(&format!("Streaming logs (pid {pid})"));
    let stream = runner
        .spawn_streaming(
            "adb",
            &["-s", &device.id, "logcat", "--pid", &pid],
            None,
            &[],
        )
        .with_context(|| format!("spawning `adb logcat --pid {pid}`"))?;
    Ok(Some(AndroidLaunch {
        stream,
        package: prepared.package,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::Kind;

    /// No `--features` passthrough — byte-identical to the pre-passthrough
    /// invocation, which is what every case but an explicit passthrough test
    /// asserts against.
    const NO_EXTRA: &[String] = &[];

    fn android(id: &str, name: &str) -> Device {
        Device {
            id: id.to_string(),
            name: name.to_string(),
            platform: Platform::Android,
            kind: Kind::Emulator,
            os_version: None,
            connection_state: None,
        }
    }

    fn ios(id: &str, name: &str) -> Device {
        Device {
            id: id.to_string(),
            name: name.to_string(),
            platform: Platform::Ios,
            kind: Kind::Simulator,
            os_version: None,
            connection_state: None,
        }
    }

    #[test]
    fn no_devices_and_no_pattern_falls_back_to_desktop() {
        assert_eq!(select_device(&[], None), DeviceSelection::Desktop);
    }

    #[test]
    fn only_ios_devices_and_no_pattern_falls_back_to_desktop() {
        let devices = vec![ios("sim1", "iPhone 15")];
        assert_eq!(select_device(&devices, None), DeviceSelection::Desktop);
    }

    #[test]
    fn exactly_one_android_device_auto_picked() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        assert_eq!(
            select_device(&devices, None),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn multiple_android_devices_are_ambiguous() {
        let devices = vec![
            android("emulator-5554", "Pixel 7"),
            android("R58N90ABCDE", "Pixel 8"),
        ];
        assert_eq!(
            select_device(&devices, None),
            DeviceSelection::Ambiguous(devices.clone())
        );
    }

    #[test]
    fn dash_d_prefix_matches_id() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        assert_eq!(
            select_device(&devices, Some("emulator")),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn dash_d_prefix_matches_name_case_insensitively() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        assert_eq!(
            select_device(&devices, Some("pixel")),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn dash_d_no_match_errs() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        match select_device(&devices, Some("nope")) {
            DeviceSelection::Error(msg) => assert!(msg.contains("no device matching")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn dash_d_ambiguous_match_errs() {
        let devices = vec![
            android("emulator-5554", "Pixel 7"),
            android("emulator-5556", "Pixel 8"),
        ];
        match select_device(&devices, Some("emulator")) {
            DeviceSelection::Error(msg) => assert!(msg.contains("ambiguous")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn dash_d_matching_ios_device_is_auto_selected() {
        // `select_device` no longer special-cases iOS: platform/kind
        // dispatch (iOS simulator drive pipeline vs. a sentinel for physical
        // devices) is `commands::run::run_on_device`'s job.
        let devices = vec![ios("sim1", "iPhone 15")];
        assert_eq!(
            select_device(&devices, Some("sim1")),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn parse_prompt_selection_accepts_in_range_number() {
        assert_eq!(parse_prompt_selection("2", 3), Ok(1));
        assert_eq!(parse_prompt_selection(" 1 \n", 3), Ok(0));
    }

    #[test]
    fn parse_prompt_selection_rejects_out_of_range() {
        assert!(parse_prompt_selection("0", 3).is_err());
        assert!(parse_prompt_selection("4", 3).is_err());
    }

    #[test]
    fn parse_prompt_selection_rejects_non_numeric() {
        assert!(parse_prompt_selection("abc", 3).is_err());
    }

    #[test]
    fn home_dir_prefers_home_over_userprofile() {
        let env = crate::doctor::FakeEnv::new()
            .set("HOME", "/home/someone")
            .set("USERPROFILE", "C:\\Users\\someone");
        assert_eq!(home_dir(&env), Some(PathBuf::from("/home/someone")));
    }

    /// The Windows-only path in practice: no `HOME`, `%USERPROFILE%` set —
    /// mirrors `frust-dap`'s `vscode::home_dir` fallback.
    #[test]
    fn home_dir_falls_back_to_userprofile_when_home_unset() {
        let env = crate::doctor::FakeEnv::new().set("USERPROFILE", "C:\\Users\\someone");
        assert_eq!(home_dir(&env), Some(PathBuf::from("C:\\Users\\someone")));
    }

    #[test]
    fn home_dir_none_when_neither_set() {
        let env = crate::doctor::FakeEnv::new();
        assert_eq!(home_dir(&env), None);
    }

    /// The leg this crate's `home_dir` gained by delegating to
    /// `host_path::home_dir_from`: previously the chain stopped at
    /// `USERPROFILE`, so `HOMEDRIVE`+`HOMEPATH` alone used to resolve to
    /// `None` here (unlike `frust-dap`'s `vscode::home_dir`, which already
    /// had it).
    #[test]
    fn home_dir_falls_back_to_homedrive_and_homepath() {
        let env = crate::doctor::FakeEnv::new()
            .set("HOMEDRIVE", "C:")
            .set("HOMEPATH", "\\Users\\someone");
        assert_eq!(home_dir(&env), Some(PathBuf::from("C:\\Users\\someone")));
    }

    mod run_pipeline {
        use super::*;
        use crate::build_info::{BuildArgs, BuildMode};
        use crate::doctor::FakeEnv;
        use crate::process::{FakeProcessRunner, Output};
        use std::fs;
        use std::sync::atomic::{AtomicU32, Ordering};

        const JAVA_HOME: &str = "/opt/jdk17";

        fn unique_project_dir(tag: &str) -> std::path::PathBuf {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "frust-cli-android-run-pipeline-test-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("android")).unwrap();
            fs::write(dir.join("android/gradlew"), "#!/bin/sh\n").unwrap();
            fs::write(
                dir.join("frust.toml"),
                "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n",
            )
            .unwrap();
            dir
        }

        /// The `FakeProcessRunner` key for a Frust-driven `./gradlew` invocation
        /// in the project at `project_dir`: every one now leads with
        /// `--project-cache-dir <project>/build/android/.gradle` (see
        /// `android_run::gradle::leading_args`), so a fixture has to carry it
        /// too. Registering the whole argv — the fake only falls back to a
        /// registered key that is a *prefix* of the real one — is what makes
        /// these tests assert the argv: drop or misplace the cache flag and no
        /// fixture matches at all.
        ///
        /// The leading program name is [`crate::android_run::gradle::gradle_wrapper`]'s
        /// own output — `./gradlew` everywhere but Windows, where the run
        /// pipeline spawns the absolute `gradlew.bat` wrapper — rather than a
        /// hardcoded `"./gradlew"`, so this fixture key matches what the
        /// pipeline actually invokes on every host `cargo test` runs on.
        fn gradlew_key(project_dir: &Path, task_and_props: &str) -> String {
            let android_dir = project_dir.join("android");
            format!(
                "{} --project-cache-dir {} {task_and_props}",
                crate::android_run::gradle::gradle_wrapper(&android_dir).to_string_lossy(),
                crate::android_run::gradle::project_cache_dir(project_dir).display()
            )
        }

        /// `frust run`'s pipeline always builds a single-ABI, non-split APK —
        /// the `AndroidArtifact` shape `discover_single_apk_since` searches
        /// with.
        fn apk_target() -> AndroidArtifact {
            AndroidArtifact::Apk {
                split_per_abi: false,
                abis: Vec::new(),
            }
        }

        /// The migrated `build/android/app/outputs/apk/…` directory a
        /// fixture plants an APK in — via
        /// [`crate::android_build::artifacts::expected_output_dir`] rather
        /// than a `/`-joined literal, so this matches the pure-native-
        /// separator path `discover_single_apk_since` actually resolves and
        /// hands to `adb install` on every host `cargo test` runs on (a
        /// literal like `dir.join("build/android/app/outputs/apk/debug")`
        /// pushes one component whose *own* text still carries `/`, which
        /// `Path::display` never rewrites to `\` on Windows).
        fn output_dir(dir: &Path, mode: BuildMode, flavor: Option<&str>) -> PathBuf {
            crate::android_build::artifacts::expected_output_dir(dir, &apk_target(), mode, flavor)
        }

        /// The pre-migration `android/app/build/outputs/apk/…` directory —
        /// see [`output_dir`].
        fn legacy_dir(dir: &Path, mode: BuildMode, flavor: Option<&str>) -> PathBuf {
            crate::android_build::artifacts::legacy_output_dir(dir, &apk_target(), mode, flavor)
        }

        /// Complete release signing material for a default-configured
        /// project: the four values `signing::check_release_signing`
        /// resolves, plus a placeholder file at the `storeFile` path they
        /// name (the gate checks a keystore exists there, not that it is a
        /// valid JKS).
        fn write_release_signing(android_dir: &Path) {
            fs::write(android_dir.join("upload.jks"), b"not-a-real-jks").unwrap();
            fs::write(
                android_dir.join("key.properties"),
                "storePassword=pw\nkeyPassword=pw\nkeyAlias=upload\nstoreFile=upload.jks\n",
            )
            .unwrap();
        }

        fn fake_env() -> FakeEnv {
            FakeEnv::new().set("JAVA_HOME", JAVA_HOME)
        }

        fn ok(stdout: &str) -> Output {
            Output {
                success: true,
                stdout: stdout.to_string(),
                stderr: String::new(),
            }
        }

        /// Every preflight check `preflight::run` performs (including the
        /// device-only `adb version` one this module's pipeline needs, unlike
        /// `android_build`'s device-less preflight).
        fn preflight_ok_runner() -> FakeProcessRunner {
            FakeProcessRunner::new()
                .with(
                    "rustup target list --installed",
                    ok("aarch64-linux-android\n"),
                )
                .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
                .with(
                    format!("{JAVA_HOME}/bin/java -version"),
                    Output {
                        success: true,
                        stdout: String::new(),
                        stderr: "openjdk version \"17.0.9\" 2023-10-17\n".to_string(),
                    },
                )
                .with("adb version", ok("Android Debug Bridge version 1.0.41\n"))
        }

        fn device() -> Device {
            Device {
                id: "emulator-5554".to_string(),
                name: "Pixel 7".to_string(),
                platform: Platform::Android,
                kind: Kind::Emulator,
                os_version: None,
                connection_state: None,
            }
        }

        fn info(mode: BuildMode, flavor: Option<&str>) -> BuildInfo {
            BuildInfo::from_args(
                BuildArgs {
                    flavor: flavor.map(str::to_string),
                    ..BuildArgs::default()
                },
                mode,
            )
            .unwrap()
        }

        /// Registering the exact `adb install` invocation to *fail* (rather
        /// than succeed) stops the pipeline right after it, before
        /// `resolve_pid`'s real-clock retry loop / `ctrlc::set_handler`
        /// (which can only be installed once per test process) — a
        /// deliberately-failing fixture that only matches on the *exact*
        /// apk path proves the gradle task/props/apk-path computation was
        /// correct up to that point (any mismatch earlier would instead
        /// error on an unregistered `./gradlew`/`adb` invocation).
        fn stop_after_install(runner: FakeProcessRunner, apk_path: &str) -> FakeProcessRunner {
            runner.with(
                format!("adb -s emulator-5554 install -r {apk_path}"),
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "INSTALL_FAILED_TEST_STOP".to_string(),
                },
            )
        }

        #[test]
        fn debug_default_assembles_debug_and_installs_debug_apk_unchanged() {
            let dir = unique_project_dir("debug-default");
            let android_dir = dir.join("android");
            let out_dir = output_dir(&dir, BuildMode::Debug, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();
            let apk_path = out_dir.join("app-debug.apk").to_string_lossy().into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    gradlew_key(
                        &dir,
                        "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
                    ),
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let build_info = info(BuildMode::Debug, None);
            let err = run_with_env(&runner, &dir, &device(), &build_info, NO_EXTRA, &fake_env())
                .unwrap_err();
            assert!(err.to_string().contains("adb install"), "{err}");

            let local_props = fs::read_to_string(android_dir.join("local.properties")).unwrap();
            assert!(local_props.contains("frust.versionName=1.0"));
            assert!(local_props.contains("frust.versionCode=1"));

            let _ = fs::remove_dir_all(&dir);
        }

        /// The legacy fixture for the `frust run` lane: a project whose
        /// generated Gradle config predates the `build/android/` redirect
        /// still writes its APK to AGP's own
        /// `android/app/build/outputs/…`. `prepare_session` must install
        /// THAT file — the fixture matches only on its exact path — and warn
        /// once, naming the migration recipe.
        #[test]
        fn pre_migration_layout_still_installs_and_warns_once() {
            let dir = unique_project_dir("legacy-layout-run");
            let out_dir = legacy_dir(&dir, BuildMode::Debug, None);
            fs::create_dir_all(&out_dir).unwrap();
            let planted_apk = out_dir.join("app-debug.apk");
            fs::write(&planted_apk, b"fake").unwrap();
            // The freshness gate on the legacy fallback rejects anything
            // older than `prepare_session`'s own Gradle-invocation start
            // instant; this fixture plants the file before that call (the
            // fake `./gradlew` never touches the filesystem), so its mtime
            // is pinned into the future to stand in for "this run's Gradle
            // just (re)built it". Opened for write, not `File::open`'s
            // read-only handle: `set_modified` needs `FILE_WRITE_ATTRIBUTES`
            // on the handle to succeed on Windows (`Access is denied`, os
            // error 5) — unlike Unix's path-based `utimes`, the timestamp
            // update there goes through the open handle.
            fs::OpenOptions::new()
                .write(true)
                .open(&planted_apk)
                .unwrap()
                .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
                .unwrap();
            let apk_path = planted_apk.to_string_lossy().into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    gradlew_key(
                        &dir,
                        "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
                    ),
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let never = AtomicBool::new(false);
            let mut lines = Vec::new();
            let outcome = prepare_session(
                &runner,
                &dir,
                &device(),
                &info(BuildMode::Debug, None),
                NO_EXTRA,
                &fake_env(),
                &mut |l| lines.push(l.to_string()),
                &never,
            );
            let err = match outcome {
                Err(err) => err,
                Ok(_) => panic!("expected the `adb install` fixture to stop the pipeline"),
            };
            assert!(err.to_string().contains("adb install"), "{err}");

            let warnings: Vec<&String> = lines
                .iter()
                .filter(|l| l.contains("pre-migration path"))
                .collect();
            assert_eq!(warnings.len(), 1, "{lines:?}");
            assert!(
                warnings[0].contains(crate::android_build::artifacts::MIGRATION_RECIPE_DOC),
                "{warnings:?}"
            );

            let _ = fs::remove_dir_all(&dir);
        }

        /// The freshness gate on `frust run`'s legacy fallback: a
        /// pre-migration APK that predates this run's own Gradle invocation
        /// (never touched, since the fixture's `./gradlew` is a fake that
        /// writes nothing — exactly what an UP-TO-DATE Gradle packaging task
        /// does on a source-unchanged rerun) is still installed, with a
        /// warning that it predates this build; it must never be a hard
        /// error, or every second `frust run` of an unmigrated app fails.
        #[test]
        fn pre_migration_layout_still_installs_a_stale_apk_with_a_warning() {
            let dir = unique_project_dir("legacy-layout-stale");
            let out_dir = legacy_dir(&dir, BuildMode::Debug, None);
            fs::create_dir_all(&out_dir).unwrap();
            let apk_path = out_dir.join("app-debug.apk");
            fs::write(&apk_path, b"fake").unwrap();
            // Predates any `SystemTime::now()` captured during this test.
            // Opened for write — see `pre_migration_layout_still_installs_and_warns_once`
            // for why `File::open`'s read-only handle fails `set_modified` on
            // Windows.
            let stale = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
            fs::OpenOptions::new()
                .write(true)
                .open(&apk_path)
                .unwrap()
                .set_modified(stale)
                .unwrap();

            let apk_str = apk_path.to_string_lossy().into_owned();
            let runner = stop_after_install(
                preflight_ok_runner().with(
                    gradlew_key(
                        &dir,
                        "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
                    ),
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_str,
            );

            let never = AtomicBool::new(false);
            let mut lines = Vec::new();
            let outcome = prepare_session(
                &runner,
                &dir,
                &device(),
                &info(BuildMode::Debug, None),
                NO_EXTRA,
                &fake_env(),
                &mut |l| lines.push(l.to_string()),
                &never,
            );
            // The pipeline reached `adb install` with the stale APK (the
            // fixture stops it there): a source-unchanged rerun of an
            // unmigrated app keeps working.
            let err = match outcome {
                Err(err) => err,
                Ok(_) => panic!("expected the `adb install` fixture to stop the pipeline"),
            };
            assert!(err.to_string().contains("adb install"), "{err}");
            let stale_warnings: Vec<&String> = lines
                .iter()
                .filter(|l| l.contains("predates this build"))
                .collect();
            assert_eq!(stale_warnings.len(), 1, "{lines:?}");
            assert!(stale_warnings[0].contains("frust clean"), "{lines:?}");
            assert!(
                lines
                    .iter()
                    .any(|l| l.contains(crate::android_build::artifacts::MIGRATION_RECIPE_DOC)),
                "{lines:?}"
            );

            let _ = fs::remove_dir_all(&dir);
        }

        /// The build-lane warning's `frust run` twin: on a MIGRATED project
        /// (its own `android/app/build.gradle.kts` carries the r1-04
        /// `setSrcDirs` redirect) a leftover `android/app/src/main/jniLibs`
        /// is warned about exactly once, naming the migration recipe.
        #[test]
        fn prepare_session_warns_once_about_a_legacy_jni_libs_leftover() {
            let dir = unique_project_dir("legacy-jnilibs-run");
            let out_dir = output_dir(&dir, BuildMode::Debug, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();
            let apk_path = out_dir.join("app-debug.apk").to_string_lossy().into_owned();
            let jni_dir = dir.join("android/app/src/main/jniLibs/arm64-v8a");
            fs::create_dir_all(&jni_dir).unwrap();
            fs::write(jni_dir.join("libapp.so"), b"stale").unwrap();
            fs::create_dir_all(dir.join("android/app")).unwrap();
            fs::write(
                dir.join("android/app/build.gradle.kts"),
                format!(
                    "sourceSets.getByName(\"main\").jniLibs.setSrcDirs(listOf(\"../../{}\"))\n",
                    crate::build_dirs::BuildLayout::android_jni_libs().display()
                ),
            )
            .unwrap();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    gradlew_key(
                        &dir,
                        "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
                    ),
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let never = AtomicBool::new(false);
            let mut lines = Vec::new();
            let outcome = prepare_session(
                &runner,
                &dir,
                &device(),
                &info(BuildMode::Debug, None),
                NO_EXTRA,
                &fake_env(),
                &mut |l| lines.push(l.to_string()),
                &never,
            );
            let err = match outcome {
                Err(err) => err,
                Ok(_) => panic!("expected the `adb install` fixture to stop the pipeline"),
            };
            assert!(err.to_string().contains("adb install"), "{err}");

            let warnings: Vec<&String> = lines
                .iter()
                .filter(|l| l.contains("jniLibs is a pre-build/ layout leftover"))
                .collect();
            assert_eq!(warnings.len(), 1, "{lines:?}");
            assert!(
                warnings[0].contains(crate::android_build::artifacts::MIGRATION_RECIPE_DOC),
                "{warnings:?}"
            );

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn release_assembles_release_and_installs_release_apk() {
            let dir = unique_project_dir("release-default");
            let android_dir = dir.join("android");
            write_release_signing(&android_dir);
            let out_dir = output_dir(&dir, BuildMode::Release, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();
            let apk_path = out_dir
                .join("app-release.apk")
                .to_string_lossy()
                .into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    gradlew_key(&dir, "assembleRelease -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false"),
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let build_info = info(BuildMode::Release, None);
            let err = run_with_env(&runner, &dir, &device(), &build_info, NO_EXTRA, &fake_env())
                .unwrap_err();
            assert!(err.to_string().contains("adb install"), "{err}");

            let _ = fs::remove_dir_all(&dir);
        }

        /// Legacy direction: a `--release` run against an app whose
        /// Cargo.toml declares no `lean` feature drops it and warns once
        /// through this session's `on_line` sink; the Gradle invocation carries
        /// no `-Pfrust.cargoFeatures` prop (the fixture is registered without
        /// it), so a regression that kept `lean` would surface via the absent
        /// warning. Stops at `adb install` like the sibling tests.
        #[test]
        fn release_legacy_app_drops_lean_and_warns() {
            let dir = unique_project_dir("f2-legacy");
            let android_dir = dir.join("android");
            write_release_signing(&android_dir);
            fs::write(dir.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
            let out_dir = output_dir(&dir, BuildMode::Release, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();
            let apk_path = out_dir
                .join("app-release.apk")
                .to_string_lossy()
                .into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    gradlew_key(&dir, "assembleRelease -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false"),
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let build_info = info(BuildMode::Release, None);
            let never = std::sync::atomic::AtomicBool::new(false);
            let mut lines = Vec::new();
            let err = prepare_session(
                &runner,
                &dir,
                &device(),
                &build_info,
                NO_EXTRA,
                &fake_env(),
                &mut |l| lines.push(l.to_string()),
                &never,
            )
            .err()
            .expect("expected adb install failure");
            assert!(err.to_string().contains("adb install"), "{err}");
            assert!(
                lines.iter().any(|l| l.contains("lean")),
                "legacy release run must warn about the missing `lean` feature: {lines:?}"
            );

            let _ = fs::remove_dir_all(&dir);
        }

        /// Declaring direction: an app that declares `lean` keeps it — the
        /// Gradle invocation carries `-Pfrust.cargoFeatures=bGVhbg==` (base64
        /// "lean"), registered exactly, so a regression that dropped it
        /// would produce a shorter, non-matching argv and error early — and
        /// warns nothing.
        #[test]
        fn release_declaring_app_keeps_lean_without_warning() {
            let dir = unique_project_dir("f2-declaring");
            let android_dir = dir.join("android");
            write_release_signing(&android_dir);
            fs::write(
                dir.join("Cargo.toml"),
                "[package]\nname = \"app\"\n\n[features]\nlean = [\"log/release_max_level_warn\"]\n",
            )
            .unwrap();
            let out_dir = output_dir(&dir, BuildMode::Release, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();
            let apk_path = out_dir
                .join("app-release.apk")
                .to_string_lossy()
                .into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    gradlew_key(
                        &dir,
                        "assembleRelease -Pfrust.targetPlatforms=arm64-v8a \
-Pfrust.splitPerAbi=false -Pfrust.cargoFeatures=bGVhbg==",
                    ),
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let build_info = info(BuildMode::Release, None);
            let never = std::sync::atomic::AtomicBool::new(false);
            let mut lines = Vec::new();
            let err = prepare_session(
                &runner,
                &dir,
                &device(),
                &build_info,
                NO_EXTRA,
                &fake_env(),
                &mut |l| lines.push(l.to_string()),
                &never,
            )
            .err()
            .expect("expected adb install failure");
            assert!(err.to_string().contains("adb install"), "{err}");
            assert!(
                !lines.iter().any(|l| l.contains("lean")),
                "a declaring app must not warn: {lines:?}"
            );

            let _ = fs::remove_dir_all(&dir);
        }

        /// **The backstop on the `frust run --release` side.** The gate
        /// resolved material and wrote `.frust-signing.properties`, Gradle
        /// exited 0 — and said it debug-signed anyway. `prepare_session` must
        /// refuse before the APK reaches a device. No `adb install` fixture is
        /// registered, so a regression that carried on would fail with an
        /// unregistered-invocation error instead of this message.
        #[test]
        fn release_bails_when_gradle_reports_it_debug_signed() {
            let dir = unique_project_dir("gradle-debug-signed");
            let android_dir = dir.join("android");
            write_release_signing(&android_dir);
            let out_dir = output_dir(&dir, BuildMode::Release, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();

            let runner = preflight_ok_runner().with(
                gradlew_key(
                    &dir,
                    "assembleRelease -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
                ),
                ok(
                    "> Task :app:assembleRelease\nFrust: release build is debug-signed. \
[FRUST-SIGNING-FALLBACK] Build with `frust build apk --release` …\nBUILD SUCCESSFUL",
                ),
            );

            let build_info = info(BuildMode::Release, None);
            let err = run_with_env(&runner, &dir, &device(), &build_info, NO_EXTRA, &fake_env())
                .unwrap_err();
            let message = err.to_string();
            assert!(message.contains("Gradle debug-signed"), "{message}");
            assert!(message.contains("external = true"), "{message}");

            let _ = fs::remove_dir_all(&dir);
        }

        /// The `external = true` waiver is a declared bypass and must not start
        /// hard failing on the same marker: the run proceeds to `adb install`
        /// (where the shared stop fixture ends it) and the waiver warning still
        /// fires.
        #[test]
        fn external_signing_still_proceeds_when_gradle_reports_debug_signing() {
            let dir = unique_project_dir("external-debug-signed");
            fs::write(
                dir.join("frust.toml"),
                "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[signing]\nexternal = true\n",
            )
            .unwrap();
            let out_dir = output_dir(&dir, BuildMode::Release, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();
            let apk_path = out_dir
                .join("app-release.apk")
                .to_string_lossy()
                .into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    gradlew_key(&dir, "assembleRelease -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false"),
                    ok("Frust: release build is debug-signed. [FRUST-SIGNING-FALLBACK]\n\
                        BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let build_info = info(BuildMode::Release, None);
            let never = AtomicBool::new(false);
            let mut lines = Vec::new();
            let err = prepare_session(
                &runner,
                &dir,
                &device(),
                &build_info,
                NO_EXTRA,
                &fake_env(),
                &mut |l| lines.push(l.to_string()),
                &never,
            )
            .err()
            .expect("expected the adb install stop fixture, not a signing refusal");
            assert!(err.to_string().contains("adb install"), "{err}");
            assert!(
                lines.iter().any(|l| l.contains("external = true")),
                "the waiver must still announce itself: {lines:?}"
            );

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn profile_assembles_profile_and_installs_profile_apk() {
            let dir = unique_project_dir("profile-default");
            let out_dir = output_dir(&dir, BuildMode::Profile, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-profile.apk"), b"fake").unwrap();
            let apk_path = out_dir
                .join("app-profile.apk")
                .to_string_lossy()
                .into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    gradlew_key(&dir, "assembleProfile -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false"),
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let build_info = info(BuildMode::Profile, None);
            let err = run_with_env(&runner, &dir, &device(), &build_info, NO_EXTRA, &fake_env())
                .unwrap_err();
            assert!(err.to_string().contains("adb install"), "{err}");

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn flavor_and_release_assembles_flavored_task_and_installs_flavored_apk() {
            let dir = unique_project_dir("flavor-release");
            let android_dir = dir.join("android");
            write_release_signing(&android_dir);
            let out_dir = output_dir(&dir, BuildMode::Release, Some("paid"));
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-paid-release.apk"), b"fake").unwrap();
            let apk_path = out_dir
                .join("app-paid-release.apk")
                .to_string_lossy()
                .into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    gradlew_key(&dir, "assemblePaidRelease -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false"),
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let build_info = info(BuildMode::Release, Some("paid"));
            let err = run_with_env(&runner, &dir, &device(), &build_info, NO_EXTRA, &fake_env())
                .unwrap_err();
            assert!(err.to_string().contains("adb install"), "{err}");

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn release_without_key_properties_errs_before_invoking_gradle() {
            let dir = unique_project_dir("release-no-keystore");
            // No `./gradlew`/`adb` fixtures registered at all: an unexpected
            // call would itself error, proving Gradle is never invoked.
            let runner = preflight_ok_runner();

            let build_info = info(BuildMode::Release, None);
            let err = run_with_env(&runner, &dir, &device(), &build_info, NO_EXTRA, &fake_env())
                .unwrap_err();
            assert!(err.to_string().contains("keytool"), "{err}");

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn device_abi_flows_into_target_platforms_property() {
            let dir = unique_project_dir("device-abi-x86-64");
            let out_dir = output_dir(&dir, BuildMode::Debug, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();
            let apk_path = out_dir.join("app-debug.apk").to_string_lossy().into_owned();

            let runner = stop_after_install(
                preflight_ok_runner()
                    .with(
                        "adb -s emulator-5554 shell getprop ro.product.cpu.abi",
                        ok("x86_64\n"),
                    )
                    .with(
                        gradlew_key(&dir, "assembleDebug -Pfrust.targetPlatforms=x86_64 -Pfrust.splitPerAbi=false"),
                        ok("BUILD SUCCESSFUL"),
                    ),
                &apk_path,
            );

            let build_info = info(BuildMode::Debug, None);
            let err = run_with_env(&runner, &dir, &device(), &build_info, NO_EXTRA, &fake_env())
                .unwrap_err();
            assert!(err.to_string().contains("adb install"), "{err}");

            let _ = fs::remove_dir_all(&dir);
        }

        /// Plants a fake SDK (one build-tools version holding an `aapt2`)
        /// under `dir`, returning the `aapt2` path the badging step will
        /// invoke and the `ANDROID_HOME` value pointing at its SDK root.
        fn plant_aapt2(dir: &std::path::Path) -> (String, String) {
            // Component-wise `Path::join`s, not a `/`-joined literal: they
            // must render exactly like `badging::locate_aapt2`'s own
            // real-directory-read `.join()` chain, or the fake runner's
            // `dump badging` key (built from this return value) never
            // matches what the pipeline actually spawns on Windows. The
            // binary name mirrors `badging::AAPT2_BIN` (`aapt2.exe` on
            // Windows) for the same reason: `locate_aapt2` only considers a
            // version directory that holds *that* file.
            let aapt2_bin = if cfg!(windows) { "aapt2.exe" } else { "aapt2" };
            let build_tools = dir.join("sdk").join("build-tools").join("35.0.1");
            fs::create_dir_all(&build_tools).unwrap();
            let aapt2 = build_tools.join(aapt2_bin);
            fs::write(&aapt2, "#!/bin/sh\n").unwrap();
            (
                aapt2.to_string_lossy().into_owned(),
                dir.join("sdk").to_string_lossy().into_owned(),
            )
        }

        /// The flavor bug this badging step exists for: a `dev` flavor
        /// declaring `applicationIdSuffix ".dev"` installs
        /// `dev.f0x.myapp.dev`, while AGP roots the launchable activity at
        /// the module namespace (`dev.f0x.myapp.MainActivity`). Only the
        /// badging-derived component and package are registered on the fake
        /// runner, so a regression back to the `frust.toml`-derived
        /// `dev.f0x.myapp/.MainActivity` + `pidof dev.f0x.myapp` would find
        /// no fixture and error instead of silently passing.
        #[test]
        fn badging_identity_drives_both_the_launch_component_and_the_pid_poll() {
            let dir = unique_project_dir("badging-flavor");
            let out_dir = output_dir(&dir, BuildMode::Debug, Some("dev"));
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-dev-debug.apk"), b"fake").unwrap();
            let apk_path = out_dir
                .join("app-dev-debug.apk")
                .to_string_lossy()
                .into_owned();
            let (aapt2, android_home) = plant_aapt2(&dir);

            let runner = preflight_ok_runner()
                .with(
                    gradlew_key(&dir, "assembleDevDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false"),
                    ok("BUILD SUCCESSFUL"),
                )
                .with(format!("adb -s emulator-5554 install -r {apk_path}"), ok(""))
                .with(
                    format!("{aapt2} dump badging {apk_path}"),
                    ok("package: name='dev.f0x.myapp.dev' versionCode='1' versionName='1.0'\n\
                        launchable-activity: name='dev.f0x.myapp.MainActivity'  label='' icon=''\n"),
                )
                .with(
                    "adb -s emulator-5554 shell am start -n dev.f0x.myapp.dev/dev.f0x.myapp.MainActivity",
                    ok(""),
                )
                .with(
                    "adb -s emulator-5554 shell pidof dev.f0x.myapp.dev",
                    ok("4242\n"),
                );

            let never = AtomicBool::new(false);
            let mut lines = Vec::new();
            let prepared = prepare_session(
                &runner,
                &dir,
                &device(),
                &info(BuildMode::Debug, Some("dev")),
                NO_EXTRA,
                &fake_env().set("ANDROID_HOME", &android_home),
                &mut |l| lines.push(l.to_string()),
                &never,
            )
            .unwrap()
            .expect("a prepared session, not a cancellation");

            assert_eq!(prepared.pid, "4242");
            assert!(
                lines.iter().any(|l| l == "Launching dev.f0x.myapp.dev…"),
                "the launch line must name the installed package: {lines:?}"
            );
            assert!(
                !lines.iter().any(|l| l.starts_with("warning:")),
                "a readable APK must not warn: {lines:?}"
            );

            let _ = fs::remove_dir_all(&dir);
        }

        /// Unreadable badging (here: no SDK env, so the PATH `aapt2` lookup
        /// fails to spawn) warns once and falls back to the pre-badging
        /// `<app_id>/.MainActivity` + `pidof <app_id>` behavior — never a hard
        /// error. Only the fallback invocations are registered, so a
        /// regression that errored out (or launched something else) fails.
        #[test]
        fn unreadable_badging_warns_once_and_falls_back_to_the_frust_toml_identity() {
            let dir = unique_project_dir("badging-fallback");
            let out_dir = output_dir(&dir, BuildMode::Debug, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();
            let apk_path = out_dir.join("app-debug.apk").to_string_lossy().into_owned();

            let runner = preflight_ok_runner()
                .with(
                    gradlew_key(
                        &dir,
                        "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
                    ),
                    ok("BUILD SUCCESSFUL"),
                )
                .with(
                    format!("adb -s emulator-5554 install -r {apk_path}"),
                    ok(""),
                )
                .with(
                    "adb -s emulator-5554 shell am start -n dev.f0x.myapp/.MainActivity",
                    ok(""),
                )
                .with(
                    "adb -s emulator-5554 shell pidof dev.f0x.myapp",
                    ok("4242\n"),
                );

            let never = AtomicBool::new(false);
            let mut lines = Vec::new();
            let prepared = prepare_session(
                &runner,
                &dir,
                &device(),
                &info(BuildMode::Debug, None),
                NO_EXTRA,
                &fake_env(),
                &mut |l| lines.push(l.to_string()),
                &never,
            )
            .unwrap()
            .expect("a prepared session, not a cancellation");

            assert_eq!(prepared.pid, "4242");
            assert_eq!(
                lines.iter().filter(|l| l.starts_with("warning:")).count(),
                1,
                "exactly one fallback warning: {lines:?}"
            );
            assert!(
                lines.iter().any(|l| l == "Launching dev.f0x.myapp…"),
                "{lines:?}"
            );

            let _ = fs::remove_dir_all(&dir);
        }

        /// The TUI streaming seam drives the same build→install→launch core and
        /// hands back a live, drainable logcat [`StreamHandle`] — the phase
        /// lines (including the `Streaming logs (pid …)` marker) arrive through
        /// `on_line`, the logcat lines through the handle.
        #[test]
        fn spawn_session_reaches_a_drainable_logcat_stream() {
            let dir = unique_project_dir("spawn-happy");
            let out_dir = output_dir(&dir, BuildMode::Debug, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();
            let apk_path = out_dir.join("app-debug.apk").to_string_lossy().into_owned();

            let runner = preflight_ok_runner()
                .with(
                    gradlew_key(
                        &dir,
                        "assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
                    ),
                    ok("BUILD SUCCESSFUL"),
                )
                .with(
                    format!("adb -s emulator-5554 install -r {apk_path}"),
                    ok(""),
                )
                .with(
                    "adb -s emulator-5554 shell am start -n dev.f0x.myapp/.MainActivity",
                    ok(""),
                )
                .with(
                    "adb -s emulator-5554 shell pidof dev.f0x.myapp",
                    ok("4242\n"),
                )
                .with_stream(
                    "adb -s emulator-5554 logcat --pid 4242",
                    ["D/frust: hello", "D/frust: world"],
                    true,
                );

            let cancel = AtomicBool::new(false);
            let mut lines = Vec::new();
            let launch = spawn_session_with_env(
                &runner,
                &dir,
                &device(),
                &info(BuildMode::Debug, None),
                &mut |l| lines.push(l.to_string()),
                &cancel,
                &fake_env(),
            )
            .unwrap()
            .expect("a live logcat handle, not a cancellation");

            assert!(lines.iter().any(|l| l.starts_with("Building")), "{lines:?}");
            assert!(
                lines.iter().any(|l| l == "Streaming logs (pid 4242)"),
                "{lines:?}"
            );
            // The *installed* package rides back with the stream, so a caller
            // wanting `am force-stop` never has to re-parse the launch line.
            assert_eq!(launch.package, "dev.f0x.myapp");

            let mut handle = launch.stream;
            let mut logcat = Vec::new();
            while let Ok(line) = handle.lines.recv() {
                logcat.push(line);
            }
            assert_eq!(logcat, vec!["D/frust: hello", "D/frust: world"]);
            assert!(handle.wait());

            let _ = fs::remove_dir_all(&dir);
        }

        /// A cancel observed before the streaming phase (here: from the start,
        /// tripping the first post-preflight boundary check) abandons the
        /// pipeline with `Ok(None)` and never spawns a logcat stream — no
        /// gradle/adb fixtures are registered, proving the build phase is never
        /// reached.
        #[test]
        fn spawn_session_cancelled_before_streaming_returns_none() {
            let dir = unique_project_dir("spawn-cancel");
            let runner = preflight_ok_runner();
            let cancel = AtomicBool::new(true);
            let out = spawn_session_with_env(
                &runner,
                &dir,
                &device(),
                &info(BuildMode::Debug, None),
                &mut |_l| {},
                &cancel,
                &fake_env(),
            )
            .unwrap();
            assert!(out.is_none());
            let _ = fs::remove_dir_all(&dir);
        }

        /// A hot session's stage step runs after preflight with the device
        /// ABI, before Gradle, and Gradle then assembles with `-x
        /// cargoNdkBuild` — the only `./gradlew` fixture registered, so the
        /// plain `assembleDebug` argv would match nothing.
        #[test]
        fn a_hot_session_stages_the_library_then_assembles_without_cargo_ndk_build() {
            let dir = unique_project_dir("hot-stage");
            let out_dir = output_dir(&dir, BuildMode::Debug, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();
            let apk_path = out_dir.join("app-debug.apk").to_string_lossy().into_owned();
            let runner = stop_after_install(
                preflight_ok_runner().with(
                    gradlew_key(
                        &dir,
                        "assembleDebug -x cargoNdkBuild -Pfrust.targetPlatforms=arm64-v8a \
                         -Pfrust.splitPerAbi=false",
                    ),
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let mut staged_for = Vec::new();
            let mut stage = |abi: &str, on_line: &mut dyn FnMut(&str)| -> Result<()> {
                staged_for.push(abi.to_string());
                on_line("[stage] fat library staged");
                Ok(())
            };
            let mut lines = Vec::new();
            let err = prepare_session_with(
                &runner,
                &dir,
                &device(),
                &info(BuildMode::Debug, None),
                NO_EXTRA,
                &fake_env(),
                &mut |line| lines.push(line.to_string()),
                &AtomicBool::new(false),
                NativeLib::Staged(&mut stage),
            )
            .err()
            .expect("the scripted install stops the pipeline");
            assert!(err.to_string().contains("adb install"), "{err}");
            assert_eq!(staged_for, vec!["arm64-v8a"]);
            let building = lines.iter().position(|l| l.starts_with("Building"));
            let staged = lines.iter().position(|l| l == "[stage] fat library staged");
            let finished = lines.iter().position(|l| l.starts_with("Build finished"));
            assert!(
                building < staged && staged < finished && building.is_some(),
                "{lines:?}"
            );
            let _ = fs::remove_dir_all(&dir);
        }

        /// A stage failure (a fat build that did not compile) ends the
        /// pipeline with that error before Gradle runs: no `./gradlew`
        /// fixture is registered at all.
        #[test]
        fn a_failed_stage_stops_the_hot_session_before_gradle() {
            let dir = unique_project_dir("hot-stage-fails");
            let runner = preflight_ok_runner();
            let mut stage = |_: &str, _: &mut dyn FnMut(&str)| -> Result<()> {
                bail!("the hot-patch fat build failed")
            };
            let err = prepare_session_with(
                &runner,
                &dir,
                &device(),
                &info(BuildMode::Debug, None),
                NO_EXTRA,
                &fake_env(),
                &mut |_| {},
                &AtomicBool::new(false),
                NativeLib::Staged(&mut stage),
            )
            .err()
            .expect("the stage failure ends the pipeline");
            assert_eq!(err.to_string(), "the hot-patch fat build failed");
            let _ = fs::remove_dir_all(&dir);
        }

        /// A hot session is Debug-only: a profile run that reached the
        /// staged path is refused before the stage step runs.
        #[test]
        fn a_hot_session_refuses_a_non_debug_variant_before_staging() {
            let dir = unique_project_dir("hot-stage-profile");
            let runner = preflight_ok_runner();
            let mut ran = false;
            let mut stage = |_: &str, _: &mut dyn FnMut(&str)| -> Result<()> {
                ran = true;
                Ok(())
            };
            let err = prepare_session_with(
                &runner,
                &dir,
                &device(),
                &info(BuildMode::Profile, None),
                NO_EXTRA,
                &fake_env(),
                &mut |_| {},
                &AtomicBool::new(false),
                NativeLib::Staged(&mut stage),
            )
            .err()
            .expect("a profile hot session is refused");
            assert!(err.to_string().contains("debug library"), "{err}");
            assert!(!ran);
            let _ = fs::remove_dir_all(&dir);
        }

        /// The hot spawn seam reaches the same drainable logcat stream as
        /// [`spawn_session`], through the staged build.
        #[test]
        fn spawn_hot_session_reaches_a_drainable_logcat_stream() {
            let dir = unique_project_dir("hot-spawn");
            let out_dir = output_dir(&dir, BuildMode::Debug, None);
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();
            let apk_path = out_dir.join("app-debug.apk").to_string_lossy().into_owned();
            let runner = preflight_ok_runner()
                .with(
                    gradlew_key(
                        &dir,
                        "assembleDebug -x cargoNdkBuild -Pfrust.targetPlatforms=arm64-v8a \
                         -Pfrust.splitPerAbi=false",
                    ),
                    ok("BUILD SUCCESSFUL"),
                )
                .with(
                    format!("adb -s emulator-5554 install -r {apk_path}"),
                    ok(""),
                )
                .with(
                    "adb -s emulator-5554 shell am start -n dev.f0x.myapp/.MainActivity",
                    ok(""),
                )
                .with(
                    "adb -s emulator-5554 shell pidof dev.f0x.myapp",
                    ok("4242\n"),
                )
                .with_stream(
                    "adb -s emulator-5554 logcat --pid 4242",
                    ["I/frust: hello"],
                    true,
                );
            let mut stages = 0;
            let mut stage = |_: &str, _: &mut dyn FnMut(&str)| -> Result<()> {
                stages += 1;
                Ok(())
            };
            let launch = spawn_hot_session_with_env(
                &runner,
                &dir,
                &device(),
                &info(BuildMode::Debug, None),
                &mut |_| {},
                &AtomicBool::new(false),
                &mut stage,
                &fake_env(),
            )
            .unwrap()
            .expect("a live logcat handle");
            assert_eq!(stages, 1);
            assert_eq!(launch.package, "dev.f0x.myapp");
            let mut handle = launch.stream;
            assert_eq!(handle.lines.recv().unwrap(), "I/frust: hello");
            assert!(handle.wait());
            let _ = fs::remove_dir_all(&dir);
        }
    }
}
