//! Drives `xcrun xcodebuild ... build` in a ForgeKit project's `ios/`
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

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::ios_build::xcodebuild::host_sim_arch;
use crate::process::{Output, ProcessRunner};

/// The absolute path a successful [`build`] should have produced the app
/// bundle at, given the project `root` and the `-configuration` it was built
/// with (task 66: derived from `BuildMode::xcode_configuration()`, not
/// hardcoded `Debug`) — e.g. `build/ios/Build/Products/Profile-iphonesimulator/Runner.app`.
pub fn app_bundle_path(root: &Path, configuration: &str) -> PathBuf {
    root.join(format!(
        "build/ios/Build/Products/{configuration}-iphonesimulator/Runner.app"
    ))
}

/// Runs `xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner
/// -configuration <configuration> -sdk iphonesimulator -destination
/// id=<udid> -derivedDataPath build/ios ARCHS=<host arch> build` in `root`,
/// streaming each line of output through `on_line` (prefixed
/// `[xcodebuild] `). The `ARCHS` pin (module doc comment) is unconditional —
/// every call here targets the Simulator.
pub fn build(
    runner: &dyn ProcessRunner,
    root: &Path,
    udid: &str,
    configuration: &str,
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    let mut prefixed = |line: &str| on_line(&format!("[xcodebuild] {line}"));
    let destination = format!("id={udid}");
    let archs = format!("ARCHS={}", host_sim_arch());
    runner
        .run_streaming(
            "xcrun",
            &[
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
                "build",
            ],
            Some(root),
            &[],
            &mut prefixed,
        )
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
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} build",
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
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Profile -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} build",
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
            &mut |_| {},
        )
        .unwrap();
        assert!(out.success);
    }

    #[test]
    fn build_surfaces_failure() {
        let runner = FakeProcessRunner::new().with(
            format!(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} build",
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
            &mut |_| {},
        )
        .unwrap();
        assert!(out.success);
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
