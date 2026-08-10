//! iOS drive pipelines for `frust run`: [`run`] drives the Simulator
//! (preflight → xcodebuild → `simctl install` → `simctl launch --console-pty`,
//! streamed) and [`run_physical`] drives a signed physical device (iOS 17+
//! gate → `ios_build`'s signed device build → `devicectl device install app`
//! → `devicectl device process launch --console --terminate-existing` against
//! the built bundle's own `Info.plist`-read identity, see [`bundle_id`],
//! streamed). Both mirror `android_run`'s structure and testing style.

pub mod bundle_id;
pub mod devicectl;
pub mod preflight;
pub mod project;
pub mod simctl;
pub mod xcodebuild;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::build_info::BuildInfo;
use crate::devices::Device;
use crate::ios_build::{self, IosArtifact};
use crate::process::{ProcessRunner, RealProcessRunner, StreamHandle, tail_lines};

/// Drives the full iOS simulator pipeline (preflight → xcodebuild →
/// `simctl install` → `simctl launch`) on `device`, an already-discovered
/// (and therefore already-booted) `Platform::Ios`/`Kind::Simulator` device,
/// against the Frust project rooted at `root`. `info.mode` selects the
/// `-configuration` xcodebuild builds and the matching
/// `<config>-iphonesimulator` products directory the app bundle is installed
/// from; flavor/defines/version aren't threaded here (unlike the Android
/// pipeline) — the iOS simulator run path stays scheme-fixed (`Runner`).
pub fn run(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
) -> Result<u8> {
    // The CLI path never cancels — drive the shared preflight/build/install
    // core to completion, then block on `simctl launch` under a Ctrl-C
    // handler (the TUI seam [`spawn_session`] takes the same core but returns
    // the launch stream's killable handle instead).
    let never = AtomicBool::new(false);
    let mut on_line = |line: &str| println!("{line}");
    let prepared =
        match prepare_simulator_session(runner, root, device, info, &mut on_line, &never)? {
            Some(prepared) => prepared,
            None => return Ok(0),
        };

    println!("Launching {}…", prepared.bundle_id);
    // Best-effort cleanup on Ctrl-C: `simctl launch` is a foreground bridge
    // process, not the app itself, so killing it (the default SIGINT
    // disposition) does not terminate the app running in the simulator —
    // explicitly `simctl terminate` it for deterministic cleanup. Uses a
    // fresh `RealProcessRunner` (not the injected `runner`) since the
    // handler must be `'static` and this path only ever runs for real.
    let udid_for_handler = prepared.udid.clone();
    let bundle_id_for_handler = prepared.bundle_id.clone();
    ctrlc::set_handler(move || {
        let _ = RealProcessRunner.run(
            "xcrun",
            &[
                "simctl",
                "terminate",
                &udid_for_handler,
                &bundle_id_for_handler,
            ],
        );
        std::process::exit(0);
    })
    .context("failed to install Ctrl-C handler")?;

    let mut on_launch_line = |line: &str| println!("{line}");
    let launch_out = simctl::launch(
        runner,
        &prepared.udid,
        &prepared.bundle_id,
        &mut on_launch_line,
    )?;

    // Best-effort cleanup on normal stream end too (app may already be gone).
    simctl::terminate(runner, &prepared.udid, &prepared.bundle_id);

    Ok(if launch_out.success { 0 } else { 1 })
}

/// What a preflight → build → install core resolves before the launch
/// streaming phase: the app's bundle id and the target device udid, for the
/// launch both front-ends attach.
///
/// The two pipelines resolve `bundle_id` differently on purpose. The physical
/// path reads it back out of the bundle it just installed (see [`bundle_id`]),
/// because a `--flavor` there builds a different scheme/configuration whose
/// target may rewrite `PRODUCT_BUNDLE_IDENTIFIER`. The simulator path is
/// scheme-fixed (`Runner`, stock `Debug|Profile|Release`) and threads no
/// flavor at all, so its built bundle can only carry the `frust.toml`-derived
/// id — it keeps using that directly rather than spending a `plutil` call to
/// be told the same thing.
struct PreparedIosSession {
    bundle_id: String,
    udid: String,
}

/// The shared preflight → build → install core of the iOS-simulator run
/// pipeline, feeding each phase line to `on_line` and checking `cancel` at
/// each boundary (see `android_run`'s `prepare_session` for the cancel-latency
/// contract). Returns `Ok(None)` when `cancel` was observed at a boundary.
fn prepare_simulator_session(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Option<PreparedIosSession>> {
    let project = project::detect(root)?;
    project::require_ios_dir(&project.root)?;

    let preflight_ctx = preflight::PreflightCtx {
        runner,
        udid: &device.id,
    };
    preflight::run(&preflight_ctx).map_err(|err| anyhow::anyhow!(err))?;

    if cancel.load(Ordering::SeqCst) {
        return Ok(None);
    }

    let configuration = info.mode.xcode_configuration();
    // Release-lean preflight: drop an undeclared `lean` for a legacy app,
    // warning once through this session's `on_line` sink, so the simulator
    // build never threads `--features lean` down to `cargo`. This path
    // calls `encode_features` directly (bypassing `ios_build::build`), so it
    // needs its own resolve.
    let (features, warning) =
        crate::cargo_manifest::resolve_release_features(&project.root, info.mode);
    if let Some(warning) = warning {
        on_line(&warning);
    }
    let features_b64 = crate::ios_build::encode_features(&features);

    on_line(&format!("Building `{}`…", project.bundle_id));
    let build_start = Instant::now();
    let build_out = xcodebuild::build(
        runner,
        &project.root,
        &device.id,
        configuration,
        features_b64.as_deref(),
        on_line,
    )?;
    if !build_out.success {
        bail!("{}", xcodebuild_failure_message(&build_out));
    }
    on_line(&format!(
        "Build finished in {:.1}s.",
        build_start.elapsed().as_secs_f32()
    ));

    let app_path = xcodebuild::app_bundle_path(&project.root, configuration);
    if !app_path.exists() {
        bail!(
            "app bundle not found at `{}` after `xcodebuild build`",
            app_path.display()
        );
    }
    let app_path = app_path.to_string_lossy().into_owned();

    if cancel.load(Ordering::SeqCst) {
        return Ok(None);
    }

    on_line(&format!("Installing on {}…", device.name));
    let install_out = simctl::install(runner, &device.id, &app_path)?;
    if !install_out.success {
        bail!(
            "`xcrun simctl install` failed: {}",
            install_out.stderr.trim()
        );
    }

    Ok(Some(PreparedIosSession {
        bundle_id: project.bundle_id,
        udid: device.id.clone(),
    }))
}

/// What a `spawn_session`/`spawn_physical_session` call hands back once the
/// launch stream is up: the killable/drainable [`StreamHandle`] plus the
/// bundle id that was actually launched, so a caller (e.g. a teardown path
/// wanting `simctl terminate <udid> <bundle_id>`) doesn't have to re-derive
/// it out of the `Launching <bundle_id>…` log line.
pub struct IosLaunch {
    pub stream: StreamHandle,
    pub bundle_id: String,
}

/// The streaming, cancellable variant of [`run`] for the `frust-tui`
/// supervisor: the same preflight → build → install core, then the
/// `simctl launch --console-pty` stream spawned through the cancellable
/// [`ProcessRunner::spawn_streaming`] seam, its [`StreamHandle`] (plus the
/// launched bundle id, see [`IosLaunch`]) handed back for the supervisor to
/// drain and kill. Returns `Ok(None)` when `cancel` was observed before the
/// launch stream began.
///
/// Killing the returned handle stops the `simctl launch` foreground bridge but
/// (like [`run`]'s own Ctrl-C path) does not itself terminate the app inside
/// the simulator — the supervisor's `stop` is prompt for the *stream*, the
/// same best-effort boundary the CLI has.
pub fn spawn_session(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Option<IosLaunch>> {
    let Some(prepared) = prepare_simulator_session(runner, root, device, info, on_line, cancel)?
    else {
        return Ok(None);
    };
    if cancel.load(Ordering::SeqCst) {
        return Ok(None);
    }
    on_line(&format!("Launching {}…", prepared.bundle_id));
    let stream = runner
        .spawn_streaming(
            "xcrun",
            &[
                "simctl",
                "launch",
                "--console-pty",
                &prepared.udid,
                &prepared.bundle_id,
            ],
            None,
            &[],
        )
        .with_context(|| format!("spawning `simctl launch {}`", prepared.bundle_id))?;
    Ok(Some(IosLaunch {
        stream,
        bundle_id: prepared.bundle_id,
    }))
}

/// Minimum `devicectl`-drivable iOS major version: devicectl only drives
/// iOS 17+ devices; ios-deploy (the iOS 15-16 alternative) is dead.
const MIN_DEVICECTL_IOS_MAJOR: u32 = 17;

/// Drives the full physical-iOS-device pipeline: an iOS-17+ gate
/// (devicectl's minimum), a signed device build via `ios_build::build`, then
/// `devicectl device install app` → `devicectl device process launch
/// --console --terminate-existing` (streamed) on `device`, an
/// already-discovered `Platform::Ios`/`Kind::PhysicalDevice` device, against
/// the Frust project rooted at `root`. `info.mode`/`info.flavor` select the
/// scheme and build configuration (`ios_build::schemes`), and the launch
/// targets the installed bundle's own identity rather than the
/// `frust.toml`-derived one (see [`bundle_id`]); signing is always requested
/// (`codesign: true`) regardless of mode — `run` never skips signing for a
/// physical device.
pub fn run_physical(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
) -> Result<u8> {
    let never = AtomicBool::new(false);
    let mut on_line = |line: &str| println!("{line}");
    let prepared = match prepare_physical_session(runner, root, device, info, &mut on_line, &never)?
    {
        Some(prepared) => prepared,
        None => return Ok(0),
    };

    println!("Launching {}…", prepared.bundle_id);
    let mut on_launch_line = |line: &str| println!("{line}");
    let launch_out = devicectl::launch(
        runner,
        &prepared.udid,
        &prepared.bundle_id,
        &mut on_launch_line,
    )?;
    if !launch_out.success {
        bail!(
            "{}",
            with_developer_mode_hint(format!(
                "`xcrun devicectl device process launch` failed: {}",
                launch_out.stderr.trim()
            ))
        );
    }

    Ok(0)
}

/// The shared iOS-17+ gate → signed build → install → launch-identity core of
/// the physical-iOS pipeline (mirrors [`prepare_simulator_session`]), feeding
/// each phase line to `on_line` and checking `cancel` at each boundary.
/// Returns `Ok(None)` when `cancel` was observed at a boundary.
fn prepare_physical_session(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Option<PreparedIosSession>> {
    let project = project::detect(root)?;
    project::require_ios_dir(&project.root)?;

    match os_version_major(device.os_version.as_deref()) {
        Some(major) if major >= MIN_DEVICECTL_IOS_MAJOR => {}
        _ => bail!(
            "physical-device runs need iOS {MIN_DEVICECTL_IOS_MAJOR}+ (devicectl); this device reports {}",
            device.os_version.as_deref().unwrap_or("unknown")
        ),
    }

    if cancel.load(Ordering::SeqCst) {
        return Ok(None);
    }

    on_line(&format!("Building `{}`…", project.bundle_id));
    let build_start = Instant::now();
    let artifacts = ios_build::build(
        runner,
        &project.root,
        info,
        &IosArtifact::App {
            simulator: false,
            codesign: true,
        },
        // Route the signed build's `[xcodebuild] …` stream through this
        // session's line sink, not `println!` — a TUI physical-run session
        // holds the terminal in raw mode (the tty-garbling fix).
        on_line,
    )?;
    on_line(&format!(
        "Build finished in {:.1}s.",
        build_start.elapsed().as_secs_f32()
    ));

    let app_path = artifacts
        .paths
        .first()
        .ok_or_else(|| anyhow::anyhow!("ios_build::build returned no `.app` artifact"))?
        .to_string_lossy()
        .into_owned();

    if cancel.load(Ordering::SeqCst) {
        return Ok(None);
    }

    on_line(&format!("Installing on {}…", device.name));
    let install_out = devicectl::install(runner, &device.id, &app_path)?;
    if !install_out.success {
        bail!(
            "{}",
            with_developer_mode_hint(format!(
                "`xcrun devicectl device install app` failed: {}",
                install_out.stderr.trim()
            ))
        );
    }

    // The *installed* identity, read back out of the `.app` just built and
    // installed rather than derived from `frust.toml`: a `--flavor` selects a
    // different scheme/configuration, whose target conventionally rewrites
    // `PRODUCT_BUNDLE_IDENTIFIER`, so the id `devicectl` must launch is not
    // the one `frust.toml` derives. An unreadable plist is a warning, not an
    // error — the fallback below is exactly right for an unflavored project.
    let (resolved, plist_warning) = bundle_id::resolve(runner, &app_path);
    if let Some(plist_warning) = plist_warning {
        on_line(&plist_warning);
    }

    Ok(Some(PreparedIosSession {
        bundle_id: resolved.unwrap_or(project.bundle_id),
        udid: device.id.clone(),
    }))
}

/// The streaming, cancellable variant of [`run_physical`] for the `frust-tui`
/// supervisor: the same iOS-17+ gate → signed build → install core, then the
/// `devicectl device process launch --console` stream spawned through the
/// cancellable [`ProcessRunner::spawn_streaming`] seam, its [`StreamHandle`]
/// (plus the launched bundle id, see [`IosLaunch`]) handed back. Returns
/// `Ok(None)` when `cancel` was observed before the launch stream began.
pub fn spawn_physical_session(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Option<IosLaunch>> {
    let Some(prepared) = prepare_physical_session(runner, root, device, info, on_line, cancel)?
    else {
        return Ok(None);
    };
    if cancel.load(Ordering::SeqCst) {
        return Ok(None);
    }
    on_line(&format!("Launching {}…", prepared.bundle_id));
    let stream = runner
        .spawn_streaming(
            "xcrun",
            &[
                "devicectl",
                "device",
                "process",
                "launch",
                "--device",
                &prepared.udid,
                "--console",
                "--terminate-existing",
                &prepared.bundle_id,
            ],
            None,
            &[],
        )
        .with_context(|| format!("spawning `devicectl process launch {}`", prepared.bundle_id))?;
    Ok(Some(IosLaunch {
        stream,
        bundle_id: prepared.bundle_id,
    }))
}

/// Parses the major version number out of a `devicectl` `osVersionNumber`
/// string (e.g. `"17.5.1"` -> `Some(17)`); `None` for a missing or
/// unparseable version, which the iOS-17+ gate treats as unsupported.
fn os_version_major(version: Option<&str>) -> Option<u32> {
    version?.split('.').next()?.parse().ok()
}

/// Appends the physical-device hardware hint `devicectl install`/`launch`
/// failures need: these commands fail opaquely when the device is locked,
/// not paired/trusted, or lacks Developer Mode, none of which
/// `xcodebuild`'s own signing errors cover.
fn with_developer_mode_hint(message: String) -> String {
    format!(
        "{message}\nhint: make sure the device is unlocked, paired and trusted with this Mac, \
         and has Developer Mode enabled (Settings → Privacy & Security → Developer Mode — the \
         device reboots after enabling it)."
    )
}

/// Builds the `xcodebuild` failure message, surfacing the last 50 non-empty
/// lines of both stdout AND stderr — xcodebuild reports most errors on
/// stdout, unlike `gradlew`, which is why this extends (rather than
/// verbatim-copies) the Android pipeline's `tail_lines` approach.
fn xcodebuild_failure_message(build_out: &crate::process::Output) -> String {
    let stdout_tail = tail_lines(&build_out.stdout, 50);
    let stderr_tail = tail_lines(&build_out.stderr, 50);
    let mut message = "`xcodebuild` failed".to_string();
    if !stdout_tail.is_empty() {
        message.push_str(&format!("\n--- stdout (tail) ---\n{stdout_tail}"));
    }
    if !stderr_tail.is_empty() {
        message.push_str(&format!("\n--- stderr (tail) ---\n{stderr_tail}"));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::{BuildArgs, BuildMode};
    use crate::devices::{Kind, Platform};
    use crate::ios_build::xcodebuild::host_sim_arch;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_project_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-ios-run-mod-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("ios/Runner.xcodeproj")).unwrap();
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n",
        )
        .unwrap();
        dir
    }

    fn device() -> Device {
        Device {
            id: "AAAA".to_string(),
            name: "iPhone 15".to_string(),
            platform: Platform::Ios,
            kind: Kind::Simulator,
            os_version: None,
            connection_state: None,
        }
    }

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn debug_info() -> BuildInfo {
        BuildInfo::from_args(BuildArgs::default(), BuildMode::Debug).unwrap()
    }

    fn profile_info() -> BuildInfo {
        BuildInfo::from_args(
            BuildArgs {
                profile: true,
                ..BuildArgs::default()
            },
            BuildMode::Debug,
        )
        .unwrap()
    }

    const BOOTED_JSON: &str = r#"{
        "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-17-5": [
                {"udid": "AAAA", "name": "iPhone 15", "state": "Booted", "isAvailable": true}
            ]
        }
    }"#;

    /// Failure exits before the pipeline ever reaches `simctl launch` (and
    /// therefore before `ctrlc::set_handler` is called), so this is safe to
    /// run alongside other tests in the same process.
    #[test]
    fn xcodebuild_failure_surfaces_tail_of_both_stdout_and_stderr() {
        let dir = unique_project_dir("build-fail");
        let runner = FakeProcessRunner::new()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios-sim\n"),
            )
            .with("xcrun simctl list devices --json", ok(BOOTED_JSON))
            .with(
                format!(
                    "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=ZnJ1c3QvcGVyZi10cmFjZSxmcnVzdC9kZXZ0b29scw== build",
                    host_sim_arch()
                ),
                Output {
                    success: false,
                    stdout: "note: Compiling Runner\nerror: build input file cannot be found"
                        .to_string(),
                    stderr: "xcodebuild: error: Scheme Runner is not currently configured"
                        .to_string(),
                },
            );

        let err = run(&runner, &dir, &device(), &debug_info()).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("xcodebuild"), "{message}");
        assert!(message.contains("--- stdout (tail) ---"), "{message}");
        assert!(
            message.contains("build input file cannot be found"),
            "{message}"
        );
        assert!(message.contains("--- stderr (tail) ---"), "{message}");
        assert!(
            message.contains("Scheme Runner is not currently configured"),
            "{message}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_failure_surfaces_before_any_xcodebuild_call() {
        let dir = unique_project_dir("preflight-fail");
        let runner = FakeProcessRunner::new().missing("xcode-select -p");
        // No `xcodebuild` fixture registered: if preflight didn't stop the
        // pipeline early, the next unmatched invocation would itself error
        // via `FakeProcessRunner`'s "missing" path, which would also fail
        // this test — either way this proves xcodebuild was never reached.
        let err = run(&runner, &dir, &device(), &debug_info()).unwrap_err();
        assert!(err.to_string().contains("Xcode not found"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Failure exits right after `simctl install` (a deliberately-failing
    /// fixture matched only on the *exact* Profile-configuration app path),
    /// before `ctrlc::set_handler` (which can only be installed once per
    /// test process) — proving `--profile` selects `-configuration Profile`
    /// and the matching `Profile-iphonesimulator` products directory,
    /// rather than the hardcoded `Debug` this pipeline used before.
    #[test]
    fn profile_mode_uses_profile_configuration_and_products_dir() {
        let dir = unique_project_dir("profile-mode");
        let app_dir = dir.join("build/ios/Build/Products/Profile-iphonesimulator/Runner.app");
        fs::create_dir_all(&app_dir).unwrap();
        let app_path = app_dir.to_string_lossy().into_owned();

        let runner = FakeProcessRunner::new()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios-sim\n"),
            )
            .with("xcrun simctl list devices --json", ok(BOOTED_JSON))
            .with(
                format!(
                    "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Profile -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=ZnJ1c3QvcGVyZi10cmFjZSxmcnVzdC9kZXZ0b29scw== build",
                    host_sim_arch()
                ),
                ok("Build succeeded"),
            )
            .with(
                format!("xcrun simctl install AAAA {app_path}"),
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "INSTALL_FAILED_TEST_STOP".to_string(),
                },
            );

        let err = run(&runner, &dir, &device(), &profile_info()).unwrap_err();
        assert!(err.to_string().contains("simctl install"), "{err}");

        let _ = fs::remove_dir_all(&dir);
    }

    fn release_info() -> BuildInfo {
        BuildInfo::from_args(
            BuildArgs {
                release: true,
                ..BuildArgs::default()
            },
            BuildMode::Debug,
        )
        .unwrap()
    }

    /// Legacy direction: a `--release` simulator run against an app whose
    /// Cargo.toml declares no `lean` feature drops it and warns once through
    /// this session's `on_line` sink; the xcodebuild invocation carries no
    /// `FRUST_FEATURES=` setting (the fixture omits it), so a regression that
    /// kept `lean` would surface via the absent warning. Drives
    /// `prepare_simulator_session` directly to observe the sink, stopping at
    /// `simctl install` like the sibling profile-mode test.
    #[test]
    fn release_legacy_app_drops_lean_and_warns() {
        let dir = unique_project_dir("f2-legacy");
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
        let app_dir = dir.join("build/ios/Build/Products/Release-iphonesimulator/Runner.app");
        fs::create_dir_all(&app_dir).unwrap();
        let app_path = app_dir.to_string_lossy().into_owned();

        let runner = FakeProcessRunner::new()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios-sim\n"),
            )
            .with("xcrun simctl list devices --json", ok(BOOTED_JSON))
            .with(
                format!(
                    "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} build",
                    host_sim_arch()
                ),
                ok("Build succeeded"),
            )
            .with(
                format!("xcrun simctl install AAAA {app_path}"),
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "INSTALL_FAILED_TEST_STOP".to_string(),
                },
            );

        let never = std::sync::atomic::AtomicBool::new(false);
        let mut lines = Vec::new();
        let err = prepare_simulator_session(
            &runner,
            &dir,
            &device(),
            &release_info(),
            &mut |l| lines.push(l.to_string()),
            &never,
        )
        .err()
        .expect("expected simctl install failure");
        assert!(err.to_string().contains("simctl install"), "{err}");
        assert!(
            lines.iter().any(|l| l.contains("lean")),
            "legacy release run must warn about the missing `lean` feature: {lines:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Declaring direction: an app that declares `lean` keeps it —
    /// `FRUST_FEATURES=bGVhbg==` (base64 "lean"), registered exactly, so a
    /// regression that dropped it would produce a shorter, non-matching argv
    /// and error early — and warns nothing.
    #[test]
    fn release_declaring_app_keeps_lean_without_warning() {
        let dir = unique_project_dir("f2-declaring");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\n\n[features]\nlean = [\"log/release_max_level_warn\"]\n",
        )
        .unwrap();
        let app_dir = dir.join("build/ios/Build/Products/Release-iphonesimulator/Runner.app");
        fs::create_dir_all(&app_dir).unwrap();
        let app_path = app_dir.to_string_lossy().into_owned();

        let runner = FakeProcessRunner::new()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios-sim\n"),
            )
            .with("xcrun simctl list devices --json", ok(BOOTED_JSON))
            .with(
                format!(
                    "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=bGVhbg== build",
                    host_sim_arch()
                ),
                ok("Build succeeded"),
            )
            .with(
                format!("xcrun simctl install AAAA {app_path}"),
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "INSTALL_FAILED_TEST_STOP".to_string(),
                },
            );

        let never = std::sync::atomic::AtomicBool::new(false);
        let mut lines = Vec::new();
        let err = prepare_simulator_session(
            &runner,
            &dir,
            &device(),
            &release_info(),
            &mut |l| lines.push(l.to_string()),
            &never,
        )
        .err()
        .expect("expected simctl install failure");
        assert!(err.to_string().contains("simctl install"), "{err}");
        assert!(
            !lines.iter().any(|l| l.contains("lean")),
            "a declaring app must not warn: {lines:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// [`spawn_session`]'s [`IosLaunch`] surfaces the exact bundle id
    /// `simctl launch` was invoked with — the D4-prerequisite contract a
    /// teardown path needs to later `simctl terminate <udid> <bundle_id>`
    /// without re-parsing the `Launching …` log line.
    #[test]
    fn spawn_session_returns_the_bundle_id_simctl_launch_was_given() {
        let dir = unique_project_dir("spawn-session-bundle-id");
        let app_dir = dir.join("build/ios/Build/Products/Debug-iphonesimulator/Runner.app");
        fs::create_dir_all(&app_dir).unwrap();
        let app_path = app_dir.to_string_lossy().into_owned();

        let runner = FakeProcessRunner::new()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios-sim\n"),
            )
            .with("xcrun simctl list devices --json", ok(BOOTED_JSON))
            .with(
                format!(
                    "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=ZnJ1c3QvcGVyZi10cmFjZSxmcnVzdC9kZXZ0b29scw== build",
                    host_sim_arch()
                ),
                ok("Build succeeded"),
            )
            .with(format!("xcrun simctl install AAAA {app_path}"), ok(""))
            .with_stream(
                "xcrun simctl launch --console-pty AAAA dev.f0x.myapp",
                ["hello from simulator"],
                true,
            );

        let never = AtomicBool::new(false);
        let mut lines = Vec::new();
        let mut launch = spawn_session(
            &runner,
            &dir,
            &device(),
            &debug_info(),
            &mut |l| lines.push(l.to_string()),
            &never,
        )
        .unwrap()
        .expect("a live launch, not a cancellation");

        assert_eq!(launch.bundle_id, "dev.f0x.myapp");
        let seen: Vec<String> = launch.stream.lines.iter().collect();
        assert_eq!(seen, vec!["hello from simulator"]);
        assert!(launch.stream.wait());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_xcodeproj_errs_before_any_process_call() {
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-ios-run-mod-test-no-xcodeproj-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n",
        )
        .unwrap();
        // No fixtures registered at all: proves `require_ios_dir` rejects
        // before any `ProcessRunner` call is made.
        let runner = FakeProcessRunner::new();
        let err = run(&runner, &dir, &device(), &debug_info()).unwrap_err();
        assert!(err.to_string().contains("ios/Runner.xcodeproj"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    // --- `run_physical` ---

    const PHYSICAL_LIST_JSON: &str = r#"{"project":{"name":"Runner","schemes":["Runner"],"configurations":["Debug","Profile","Release"]}}"#;

    /// A project fixture with `[ios] team` set explicitly so `ios_build::build`'s
    /// team resolution (env → toml → `security find-identity` auto-detect)
    /// settles on a known, deterministic team id without registering a
    /// `security find-identity` fixture.
    fn unique_physical_project_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-ios-run-physical-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("ios/Runner.xcodeproj")).unwrap();
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[ios]\nteam = \"TEAMID1234\"\n",
        )
        .unwrap();
        dir
    }

    fn physical_device() -> Device {
        Device {
            id: "00008110-000A2D3A3C68801E".to_string(),
            name: "Ed's iPhone".to_string(),
            platform: Platform::Ios,
            kind: crate::devices::Kind::PhysicalDevice,
            os_version: Some("17.5.1".to_string()),
            connection_state: None,
        }
    }

    fn plant_physical_app(dir: &std::path::Path) -> String {
        plant_physical_app_in(dir, "Debug")
    }

    /// Plants the built `.app` in `configuration`'s device products directory,
    /// returning its path — a flavored build lands under
    /// `<Mode>-<Flavor>-iphoneos`, not the stock `Debug-iphoneos`.
    fn plant_physical_app_in(dir: &std::path::Path, configuration: &str) -> String {
        let app_dir = dir
            .join("build/ios/Build/Products")
            .join(format!("{configuration}-iphoneos"))
            .join("Runner.app");
        fs::create_dir_all(&app_dir).unwrap();
        app_dir.to_string_lossy().into_owned()
    }

    /// The `plutil` invocation the launch-identity step makes for the `.app`
    /// at `app_path`.
    fn plutil_key(app_path: &str) -> String {
        format!("plutil -extract CFBundleIdentifier raw {app_path}/Info.plist")
    }

    /// Registers the preflight/scheme-listing fixtures every `run_physical`
    /// happy/failure-path test needs (mirrors `ios_build::mod`'s `base_runner`).
    fn physical_base_runner() -> FakeProcessRunner {
        FakeProcessRunner::new()
            .with("xcode-select -p", ok("/Applications/Xcode.app\n"))
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios\n"),
            )
            .with(
                "xcrun xcodebuild -list -json -project ios/Runner.xcodeproj",
                ok(PHYSICAL_LIST_JSON),
            )
            .with(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=ZnJ1c3QvcGVyZi10cmFjZSxmcnVzdC9kZXZ0b29scw== DEVELOPMENT_TEAM=TEAMID1234 CODE_SIGN_STYLE=Automatic CODE_SIGNING_ALLOWED=YES CODE_SIGNING_REQUIRED=YES -allowProvisioningUpdates -allowProvisioningDeviceRegistration build",
                ok("Build succeeded"),
            )
    }

    #[test]
    fn run_physical_happy_path_builds_installs_and_launches() {
        let dir = unique_physical_project_dir("happy");
        let app_path = plant_physical_app(&dir);

        let runner = physical_base_runner()
            .with(
                format!(
                    "xcrun devicectl device install app --device 00008110-000A2D3A3C68801E {app_path}"
                ),
                ok(""),
            )
            .with(
                "xcrun devicectl device process launch --device 00008110-000A2D3A3C68801E --console --terminate-existing dev.f0x.myapp",
                ok("hello from device\n"),
            );

        let code = run_physical(&runner, &dir, &physical_device(), &debug_info()).unwrap();
        assert_eq!(code, 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_physical_rejects_ios16_device_before_any_build_call() {
        let dir = unique_physical_project_dir("ios16-gate");
        let mut device = physical_device();
        device.os_version = Some("16.7.2".to_string());
        // No fixtures registered at all: proves the iOS-17+ gate stops the
        // pipeline before any `xcodebuild`/`devicectl` call is attempted.
        let runner = FakeProcessRunner::new();
        let err = run_physical(&runner, &dir, &device, &debug_info()).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("iOS 17+"), "{message}");
        assert!(message.contains("16.7.2"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_physical_rejects_device_with_unknown_os_version() {
        let dir = unique_physical_project_dir("unknown-version");
        let mut device = physical_device();
        device.os_version = None;
        let runner = FakeProcessRunner::new();
        let err = run_physical(&runner, &dir, &device, &debug_info()).unwrap_err();
        assert!(err.to_string().contains("unknown"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_physical_install_failure_surfaces_developer_mode_hint() {
        let dir = unique_physical_project_dir("install-fail");
        let app_path = plant_physical_app(&dir);

        // No `devicectl device process launch` fixture registered: proves
        // an install failure stops the pipeline before launch is attempted.
        let runner = physical_base_runner().with(
            format!(
                "xcrun devicectl device install app --device 00008110-000A2D3A3C68801E {app_path}"
            ),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "Unable to install: device locked".to_string(),
            },
        );

        let err = run_physical(&runner, &dir, &physical_device(), &debug_info()).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("devicectl device install app"),
            "{message}"
        );
        assert!(message.contains("device locked"), "{message}");
        assert!(message.contains("Developer Mode"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_physical_launch_failure_surfaces_developer_mode_hint() {
        let dir = unique_physical_project_dir("launch-fail");
        let app_path = plant_physical_app(&dir);

        let runner = physical_base_runner()
            .with(
                format!(
                    "xcrun devicectl device install app --device 00008110-000A2D3A3C68801E {app_path}"
                ),
                ok(""),
            )
            .with(
                "xcrun devicectl device process launch --device 00008110-000A2D3A3C68801E --console --terminate-existing dev.f0x.myapp",
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "Unable to launch: Developer Mode disabled".to_string(),
                },
            );

        let err = run_physical(&runner, &dir, &physical_device(), &debug_info()).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("devicectl device process launch"),
            "{message}"
        );
        assert!(message.contains("Developer Mode disabled"), "{message}");
        assert!(message.contains("Developer Mode enabled"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_physical_missing_xcodeproj_errs_before_any_process_call() {
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-ios-run-physical-test-no-xcodeproj-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n",
        )
        .unwrap();
        // No fixtures registered at all: proves `require_ios_dir` rejects
        // before any `ProcessRunner` call is made.
        let runner = FakeProcessRunner::new();
        let err = run_physical(&runner, &dir, &physical_device(), &debug_info()).unwrap_err();
        assert!(err.to_string().contains("ios/Runner.xcodeproj"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    // --- launch identity read from the built bundle's `Info.plist` ---
    //
    // The `run_physical` tests above deliberately register no `plutil`
    // fixture: they double as the "unreadable plist is never fatal" case,
    // proving the pipeline still builds → installs → launches the
    // `frust.toml`-derived id when the identity read fails. The tests below
    // cover the readable path.

    const FLAVORED_LIST_JSON: &str = r#"{"project":{"name":"Runner","schemes":["Runner","Develop"],"configurations":["Debug","Profile","Release","Debug-Develop"]}}"#;

    fn develop_flavor_info() -> BuildInfo {
        BuildInfo::from_args(
            BuildArgs {
                flavor: Some("develop".to_string()),
                ..BuildArgs::default()
            },
            BuildMode::Debug,
        )
        .unwrap()
    }

    /// [`physical_base_runner`]'s `--flavor develop` counterpart: scheme
    /// `Develop`, configuration `Debug-Develop` (`ios_build::schemes`), both
    /// listed by the `-list -json` fixture so the flavored-configuration
    /// verification passes.
    fn flavored_base_runner() -> FakeProcessRunner {
        FakeProcessRunner::new()
            .with("xcode-select -p", ok("/Applications/Xcode.app\n"))
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios\n"),
            )
            .with(
                "xcrun xcodebuild -list -json -project ios/Runner.xcodeproj",
                ok(FLAVORED_LIST_JSON),
            )
            .with(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Develop -configuration Debug-Develop -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=ZnJ1c3QvcGVyZi10cmFjZSxmcnVzdC9kZXZ0b29scw== DEVELOPMENT_TEAM=TEAMID1234 CODE_SIGN_STYLE=Automatic CODE_SIGNING_ALLOWED=YES CODE_SIGNING_REQUIRED=YES -allowProvisioningUpdates -allowProvisioningDeviceRegistration build",
                ok("Build succeeded"),
            )
    }

    /// The bug this identity read exists for: a flavor whose Xcode
    /// configuration rewrites `PRODUCT_BUNDLE_IDENTIFIER` installs
    /// `dev.f0x.myapp.develop`, so launching the `frust.toml`-derived
    /// `dev.f0x.myapp` gets `CoreDeviceError 10002 … is not installed`. Only
    /// the plist-derived launch is registered on the fake runner, so a
    /// regression back to the derived id would find no fixture and error
    /// instead of silently passing.
    #[test]
    fn flavored_run_launches_the_bundle_id_read_from_the_installed_app() {
        let dir = unique_physical_project_dir("plist-flavored");
        let app_path = plant_physical_app_in(&dir, "Debug-Develop");

        let runner = flavored_base_runner()
            .with(
                format!(
                    "xcrun devicectl device install app --device 00008110-000A2D3A3C68801E {app_path}"
                ),
                ok(""),
            )
            .with(plutil_key(&app_path), ok("dev.f0x.myapp.develop\n"))
            .with(
                "xcrun devicectl device process launch --device 00008110-000A2D3A3C68801E --console --terminate-existing dev.f0x.myapp.develop",
                ok("hello from device\n"),
            );

        let code = run_physical(&runner, &dir, &physical_device(), &develop_flavor_info()).unwrap();
        assert_eq!(code, 0);

        let _ = fs::remove_dir_all(&dir);
    }

    /// The no-flavor direction: the plist yields exactly the
    /// `frust.toml`-derived id, so the resolved identity — and therefore the
    /// launch — is byte-identical to the pre-fix behavior, with nothing
    /// warned.
    #[test]
    fn unflavored_plist_resolves_the_frust_toml_bundle_id_without_warning() {
        let dir = unique_physical_project_dir("plist-unflavored");
        let app_path = plant_physical_app(&dir);

        let runner = physical_base_runner()
            .with(
                format!(
                    "xcrun devicectl device install app --device 00008110-000A2D3A3C68801E {app_path}"
                ),
                ok(""),
            )
            .with(plutil_key(&app_path), ok("dev.f0x.myapp\n"));

        let never = AtomicBool::new(false);
        let mut lines = Vec::new();
        let prepared = prepare_physical_session(
            &runner,
            &dir,
            &physical_device(),
            &debug_info(),
            &mut |l| lines.push(l.to_string()),
            &never,
        )
        .unwrap()
        .expect("a prepared session, not a cancellation");

        assert_eq!(prepared.bundle_id, "dev.f0x.myapp");
        assert!(
            !lines.iter().any(|l| l.starts_with("warning:")),
            "a readable plist must not warn: {lines:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// An unreadable plist (here: `plutil` reports the key missing) warns
    /// exactly once through the session's sink and falls back to the
    /// `frust.toml`-derived bundle id — never a hard error.
    #[test]
    fn unreadable_plist_warns_once_and_falls_back_to_the_frust_toml_bundle_id() {
        let dir = unique_physical_project_dir("plist-unreadable");
        let app_path = plant_physical_app(&dir);

        let runner = physical_base_runner()
            .with(
                format!(
                    "xcrun devicectl device install app --device 00008110-000A2D3A3C68801E {app_path}"
                ),
                ok(""),
            )
            .with(
                plutil_key(&app_path),
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "No value at that key path or invalid key path: CFBundleIdentifier"
                        .to_string(),
                },
            );

        let never = AtomicBool::new(false);
        let mut lines = Vec::new();
        let prepared = prepare_physical_session(
            &runner,
            &dir,
            &physical_device(),
            &debug_info(),
            &mut |l| lines.push(l.to_string()),
            &never,
        )
        .unwrap()
        .expect("a prepared session, not a cancellation");

        assert_eq!(prepared.bundle_id, "dev.f0x.myapp");
        assert_eq!(
            lines.iter().filter(|l| l.starts_with("warning:")).count(),
            1,
            "exactly one fallback warning: {lines:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
