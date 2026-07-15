//! Drives `xcrun xcodebuild ... build` in a ForgeKit project's `ios/`
//! (mirrors `android_run::gradle`). The generated Xcode project's Rust build
//! phase does the actual `.a`/`.dylib` build; this module only shells out to
//! `xcodebuild` and streams its output.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::process::{Output, ProcessRunner};

/// Where a Debug/`iphonesimulator` build writes the app bundle, relative to
/// the project root (given `-derivedDataPath build/ios`).
pub const APP_BUNDLE_PATH: &str = "build/ios/Build/Products/Debug-iphonesimulator/Runner.app";

/// The absolute path a successful [`build`] should have produced the app
/// bundle at, given the project `root`.
pub fn app_bundle_path(root: &Path) -> PathBuf {
    root.join(APP_BUNDLE_PATH)
}

/// Runs `xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner
/// -configuration Debug -sdk iphonesimulator -destination id=<udid>
/// -derivedDataPath build/ios build` in `root`, streaming each line of
/// output through `on_line` (prefixed `[xcodebuild] `).
pub fn build(
    runner: &dyn ProcessRunner,
    root: &Path,
    udid: &str,
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    let mut prefixed = |line: &str| on_line(&format!("[xcodebuild] {line}"));
    let destination = format!("id={udid}");
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
                "Debug",
                "-sdk",
                "iphonesimulator",
                "-destination",
                &destination,
                "-derivedDataPath",
                "build/ios",
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
            "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios build",
            Output {
                success: true,
                stdout: "Build succeeded".to_string(),
                stderr: String::new(),
            },
        );
        let mut lines = Vec::new();
        let out = build(&runner, Path::new("/tmp/myapp"), "AAAA", &mut |line| {
            lines.push(line.to_string())
        })
        .unwrap();
        assert!(out.success);
        assert_eq!(lines, vec!["[xcodebuild] Build succeeded"]);
    }

    #[test]
    fn build_surfaces_failure() {
        let runner = FakeProcessRunner::new().with(
            "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios build",
            Output {
                success: false,
                stdout: "note: Compiling failed".to_string(),
                stderr: "error: build input file cannot be found".to_string(),
            },
        );
        let out = build(&runner, Path::new("/tmp/myapp"), "AAAA", &mut |_| {}).unwrap();
        assert!(!out.success);
    }

    #[test]
    fn app_bundle_path_joins_root_and_relative_path() {
        assert_eq!(
            app_bundle_path(Path::new("/tmp/myapp")),
            Path::new("/tmp/myapp/build/ios/Build/Products/Debug-iphonesimulator/Runner.app")
        );
    }
}
