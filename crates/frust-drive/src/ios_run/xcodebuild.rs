//! Drives `xcrun xcodebuild ... build` in a Frust project's `ios/`
//! (mirrors `android_run::gradle`). The generated Xcode project's Rust build
//! phase does the actual `.a`/`.dylib` build; this module only shells out to
//! `xcodebuild` and streams its output.
//!
//! This is always a simulator build (see [`build`]'s `-sdk iphonesimulator`),
//! so it unconditionally pins `ARCHS` to [`host_sim_arch`] like
//! `ios_build::xcodebuild`'s `Invocation::core` does for its own simulator
//! path: Release/Profile configs build every simulator arch by default (no
//! `ONLY_ACTIVE_ARCH`, unlike Debug), which links a slice the run-script's
//! single-arch Rust staticlib lacks (`ld: symbol(s) not found for
//! architecture x86_64` on Apple Silicon) — see commit `1a65272`.
//!
//! [`build_hot`] is the hot-patch fat build: the same invocation plus
//! command-line build settings, which outrank every project level, so the
//! template is untouched. [`hot_link_settings`] turns Xcode's own link of
//! the Rust staticlib into the fat link; the capture settings
//! (`hotpatch::ios_sim`) reach the "Build Rust staticlib" script phase,
//! which Xcode runs with every build setting exported to its environment.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::hotpatch::fat_link::LinkerFlavor;
use crate::ios_build::xcodebuild::host_sim_arch;
use crate::process::{Output, ProcessRunner};

/// The absolute path a successful [`build`] should have produced the app
/// bundle at, given the project `root` and the `-configuration` it was built
/// with (derived from `BuildMode::xcode_configuration()`, not hardcoded
/// `Debug`) — e.g. `build/ios/Build/Products/Profile-iphonesimulator/Runner.app`.
pub fn app_bundle_path(root: &Path, configuration: &str) -> PathBuf {
    root.join(format!(
        "build/ios/Build/Products/{configuration}-iphonesimulator/Runner.app"
    ))
}

/// Runs `xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner
/// -configuration <configuration> -sdk iphonesimulator -destination
/// id=<udid> -derivedDataPath build/ios ARCHS=<host arch> [FRUST_FEATURES=<b64>]
/// build` in `root`, streaming each line of output through `on_line`
/// (prefixed `[xcodebuild] `). The `ARCHS` pin (module doc comment) is
/// unconditional — every call here targets the Simulator. `features_b64` is
/// `ios_build::encode_features`' output: the pbxproj run-script decodes it
/// into `--features <csv>`, so a debug/profile
/// simulator run compiles instrumentation in while a release run gets the
/// `lean` log ceiling — the same seam `frust build` threads `FRUST_FEATURES`
/// through. Unlike user `--define`s, the feature set is mode-derived (never a
/// per-run user value), so threading it here doesn't reintroduce the
/// scheme-fixed run path's deliberate no-defines contract.
pub fn build(
    runner: &dyn ProcessRunner,
    root: &Path,
    udid: &str,
    configuration: &str,
    features_b64: Option<&str>,
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    build_with(
        runner,
        root,
        udid,
        configuration,
        features_b64,
        &[],
        on_line,
    )
}

/// [`build`] as the hot-patch fat build: `settings` (`NAME=value`, in
/// order) go after `FRUST_FEATURES` and before the `build` action. The
/// caller passes [`hot_link_settings`] plus the capture settings.
pub fn build_hot(
    runner: &dyn ProcessRunner,
    root: &Path,
    udid: &str,
    configuration: &str,
    features_b64: Option<&str>,
    settings: &[(String, String)],
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    let settings: Vec<String> = settings
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    build_with(
        runner,
        root,
        udid,
        configuration,
        features_b64,
        &settings,
        on_line,
    )
}

/// The template's `OTHER_LDFLAGS` default, kept by `$(inherited)`: a
/// command-line setting replaces the target's value, and `$(inherited)`
/// there resolves to it (`-l<crate>` and the `-framework` entries).
const INHERITED: &str = "$(inherited)";

/// The build settings that make Xcode's link of `Runner` the fat link:
///
/// - `OTHER_LDFLAGS`: the project's own flags, `@<force_load_rsp>` (a clang
///   response file holding [`force_load_response`]'s force-load of the
///   staticlib), and the anchor exported (which also limits the
///   executable's exports to it; the symbol table keeps every function).
///   The force-load sits in a response file because Xcode 27 reads a path
///   in `OTHER_LDFLAGS` as a link input that must exist when the build is
///   planned, before the script phase has produced the staticlib ("Build
///   input file cannot be found" on a clean build); it does not look
///   inside a response file.
/// - `DEAD_CODE_STRIPPING=NO`: no function a later patch may call is
///   dropped.
/// - `ENABLE_DEBUG_DYLIB=NO`: a Debug simulator build otherwise moves the
///   app's code out of `Runner` into `Runner.debug.dylib` (Xcode's debug
///   dylib split), and `Runner` is the image the symbol cache reads.
pub fn hot_link_settings(force_load_rsp: &Path) -> Vec<(String, String)> {
    let export = LinkerFlavor::Darwin.anchor_export_arg();
    vec![
        (
            "OTHER_LDFLAGS".to_string(),
            format!("{INHERITED} @{} {export}", force_load_rsp.display()),
        ),
        ("DEAD_CODE_STRIPPING".to_string(), "NO".to_string()),
        ("ENABLE_DEBUG_DYLIB".to_string(), "NO".to_string()),
    ]
}

/// The response file's content: `-Wl,-force_load,<archive>` on one line,
/// double-quoted with `\` and `"` escaped, so a path with spaces stays one
/// argument.
pub fn force_load_response(archive: &Path) -> String {
    let arg = format!("-Wl,-force_load,{}", archive.display());
    format!("\"{}\"\n", arg.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `build/ios/Build/Products/<configuration>-iphonesimulator/lib<crate>.a`:
/// where the script phase copies the Rust staticlib (`BUILT_PRODUCTS_DIR`).
pub fn staticlib_path(root: &Path, configuration: &str, crate_name: &str) -> PathBuf {
    root.join(format!(
        "build/ios/Build/Products/{configuration}-iphonesimulator/lib{crate_name}.a"
    ))
}

/// `Runner.app/Runner`: the executable a hot build links fat.
pub fn executable_path(root: &Path, configuration: &str) -> PathBuf {
    app_bundle_path(root, configuration).join("Runner")
}

/// `Runner.app/Runner.debug.dylib`: present only when Xcode split the app's
/// code out of [`executable_path`] (`ENABLE_DEBUG_DYLIB` not honoured).
pub fn debug_dylib_path(root: &Path, configuration: &str) -> PathBuf {
    app_bundle_path(root, configuration).join("Runner.debug.dylib")
}

fn build_with(
    runner: &dyn ProcessRunner,
    root: &Path,
    udid: &str,
    configuration: &str,
    features_b64: Option<&str>,
    settings: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    let mut prefixed = |line: &str| on_line(&format!("[xcodebuild] {line}"));
    let destination = format!("id={udid}");
    let archs = format!("ARCHS={}", host_sim_arch());
    let features_setting = features_b64.map(|f| format!("FRUST_FEATURES={f}"));
    let mut args: Vec<&str> = vec![
        "xcodebuild",
        "-project",
        "ios/Runner.xcodeproj",
        "-scheme",
        "Runner",
        "-configuration",
        configuration,
        "-sdk",
        "iphonesimulator",
        "-destination",
        &destination,
        "-derivedDataPath",
        "build/ios",
        &archs,
    ];
    if let Some(setting) = features_setting.as_deref() {
        args.push(setting);
    }
    args.extend(settings.iter().map(String::as_str));
    args.push("build");
    runner
        .run_streaming("xcrun", &args, Some(root), &[], &mut prefixed)
        .with_context(|| format!("running `xcodebuild` in `{}`", root.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::FakeProcessRunner;

    #[test]
    fn build_runs_xcodebuild_with_expected_args_and_prefixes_lines() {
        let runner = FakeProcessRunner::new().with(
            format!(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=ZnJ1c3QvcGVyZi10cmFjZQ== build",
                host_sim_arch()
            ),
            Output {
                success: true,
                stdout: "Build succeeded".to_string(),
                stderr: String::new(),
            },
        );
        let mut lines = Vec::new();
        let out = build(
            &runner,
            Path::new("/tmp/myapp"),
            "AAAA",
            "Debug",
            Some("ZnJ1c3QvcGVyZi10cmFjZQ=="),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert!(out.success);
        assert_eq!(lines, vec!["[xcodebuild] Build succeeded"]);
    }

    #[test]
    fn build_passes_through_a_non_debug_configuration() {
        let runner = FakeProcessRunner::new().with(
            format!(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Profile -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=ZnJ1c3QvcGVyZi10cmFjZQ== build",
                host_sim_arch()
            ),
            Output {
                success: true,
                stdout: "Build succeeded".to_string(),
                stderr: String::new(),
            },
        );
        let out = build(
            &runner,
            Path::new("/tmp/myapp"),
            "AAAA",
            "Profile",
            Some("ZnJ1c3QvcGVyZi10cmFjZQ=="),
            &mut |_| {},
        )
        .unwrap();
        assert!(out.success);
    }

    #[test]
    fn build_surfaces_failure() {
        let runner = FakeProcessRunner::new().with(
            format!(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=ZnJ1c3QvcGVyZi10cmFjZQ== build",
                host_sim_arch()
            ),
            Output {
                success: false,
                stdout: "note: Compiling failed".to_string(),
                stderr: "error: build input file cannot be found".to_string(),
            },
        );
        let out = build(
            &runner,
            Path::new("/tmp/myapp"),
            "AAAA",
            "Debug",
            Some("ZnJ1c3QvcGVyZi10cmFjZQ=="),
            &mut |_| {},
        )
        .unwrap();
        assert!(!out.success);
    }

    #[test]
    fn build_argv_pins_host_arch() {
        // Mirrors ios_build::xcodebuild's
        // `simulator_build_argv_has_no_signing_args_and_pins_host_arch`: a
        // Simulator run build must pin `ARCHS` to the host's native arch
        // (Release/Profile configs build every sim arch otherwise, which
        // links a slice the run-script's single-arch Rust staticlib lacks).
        let runner = FakeProcessRunner::new().with(
            format!(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=ZnJ1c3QvcGVyZi10cmFjZQ== build",
                host_sim_arch()
            ),
            Output {
                success: true,
                stdout: "Build succeeded".to_string(),
                stderr: String::new(),
            },
        );
        let out = build(
            &runner,
            Path::new("/tmp/myapp"),
            "AAAA",
            "Debug",
            Some("ZnJ1c3QvcGVyZi10cmFjZQ=="),
            &mut |_| {},
        )
        .unwrap();
        assert!(out.success);
    }

    #[test]
    fn build_omits_frust_features_when_none() {
        // With no features, the run build must match the pre-feature argv
        // exactly (no `FRUST_FEATURES=` setting) — the `None` arm.
        let runner = FakeProcessRunner::new().with(
            format!(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} build",
                host_sim_arch()
            ),
            Output {
                success: true,
                stdout: "Build succeeded".to_string(),
                stderr: String::new(),
            },
        );
        let out = build(
            &runner,
            Path::new("/tmp/myapp"),
            "AAAA",
            "Debug",
            None,
            &mut |_| {},
        )
        .unwrap();
        assert!(out.success);
    }

    #[test]
    fn build_threads_release_lean_feature_setting() {
        // A release simulator run threads the `lean` log ceiling
        // (base64("lean")) through `FRUST_FEATURES`, not perf-trace.
        let runner = FakeProcessRunner::new().with(
            format!(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=bGVhbg== build",
                host_sim_arch()
            ),
            Output {
                success: true,
                stdout: "Build succeeded".to_string(),
                stderr: String::new(),
            },
        );
        let out = build(
            &runner,
            Path::new("/tmp/myapp"),
            "AAAA",
            "Release",
            Some("bGVhbg=="),
            &mut |_| {},
        )
        .unwrap();
        assert!(out.success);
    }

    /// The hot build is the plain argv with the settings between
    /// `FRUST_FEATURES` and `build`, each one argument: the project's link
    /// flags kept through `$(inherited)`, the force-load response file, the
    /// anchor exported, dead-code stripping and the debug-dylib split off.
    #[test]
    fn build_hot_appends_the_fat_link_settings_before_the_action() {
        let mut settings = hot_link_settings(Path::new("/t/fat/force-load.rsp"));
        settings.push((
            "FRUST_HOTPATCH_CAPTURE".to_string(),
            "/tmp/scope".to_string(),
        ));
        let runner = FakeProcessRunner::new().with(
            format!(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=ZnJ1c3QvaG90cGF0Y2g= OTHER_LDFLAGS=$(inherited) @/t/fat/force-load.rsp -Wl,-exported_symbol,___frust_hotpatch_anchor DEAD_CODE_STRIPPING=NO ENABLE_DEBUG_DYLIB=NO FRUST_HOTPATCH_CAPTURE=/tmp/scope build",
                host_sim_arch()
            ),
            Output {
                success: true,
                stdout: "Build succeeded".to_string(),
                stderr: String::new(),
            },
        );
        let out = build_hot(
            &runner,
            Path::new("/tmp/myapp"),
            "AAAA",
            "Debug",
            Some("ZnJ1c3QvaG90cGF0Y2g="),
            &settings,
            &mut |_| {},
        )
        .unwrap();
        assert!(out.success);
        let archive = staticlib_path(Path::new("/tmp/my app"), "Debug", "myapp");
        assert_eq!(
            archive,
            Path::new("/tmp/my app/build/ios/Build/Products/Debug-iphonesimulator/libmyapp.a")
        );
        assert_eq!(
            force_load_response(&archive),
            "\"-Wl,-force_load,/tmp/my app/build/ios/Build/Products/Debug-iphonesimulator/libmyapp.a\"\n"
        );
        assert_eq!(
            force_load_response(Path::new("/t/a\"b\\c.a")),
            "\"-Wl,-force_load,/t/a\\\"b\\\\c.a\"\n"
        );
        assert_eq!(
            executable_path(Path::new("/tmp/myapp"), "Debug"),
            Path::new(
                "/tmp/myapp/build/ios/Build/Products/Debug-iphonesimulator/Runner.app/Runner"
            )
        );
        assert_eq!(
            debug_dylib_path(Path::new("/tmp/myapp"), "Debug"),
            Path::new(
                "/tmp/myapp/build/ios/Build/Products/Debug-iphonesimulator/Runner.app/Runner.debug.dylib"
            )
        );
    }

    #[test]
    fn app_bundle_path_joins_root_configuration_and_relative_path() {
        assert_eq!(
            app_bundle_path(Path::new("/tmp/myapp"), "Debug"),
            Path::new("/tmp/myapp/build/ios/Build/Products/Debug-iphonesimulator/Runner.app")
        );
        assert_eq!(
            app_bundle_path(Path::new("/tmp/myapp"), "Profile"),
            Path::new("/tmp/myapp/build/ios/Build/Products/Profile-iphonesimulator/Runner.app")
        );
        assert_eq!(
            app_bundle_path(Path::new("/tmp/myapp"), "Release"),
            Path::new("/tmp/myapp/build/ios/Build/Products/Release-iphonesimulator/Runner.app")
        );
    }
}
