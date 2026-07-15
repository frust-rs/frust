//! Drives `./gradlew assembleDebug` in `<app>/android/` (spec §12.4 step 4).
//! The generated Gradle project's cargo-ndk task does the actual Rust
//! `.so` build; this module only shells out to Gradle and streams its
//! output.

use std::path::Path;

use anyhow::{Context, Result};

use crate::process::{Output, ProcessRunner};

/// Where `assembleDebug` writes the debug APK, relative to `android/`.
pub const DEBUG_APK_PATH: &str = "app/build/outputs/apk/debug/app-debug.apk";

/// Runs `./gradlew assembleDebug` in `android_dir`, streaming each line of
/// output through `on_line` (prefixed `[gradle] `) and exporting
/// `JAVA_HOME=java_home` for the child process only.
pub fn assemble_debug(
    runner: &dyn ProcessRunner,
    android_dir: &Path,
    java_home: &str,
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    let mut prefixed = |line: &str| on_line(&format!("[gradle] {line}"));
    runner
        .run_streaming(
            "./gradlew",
            &["assembleDebug"],
            Some(android_dir),
            &[("JAVA_HOME", java_home)],
            &mut prefixed,
        )
        .with_context(|| {
            format!(
                "running `./gradlew assembleDebug` in `{}`",
                android_dir.display()
            )
        })
}

/// Best-effort check for whether Gradle's wrapper distribution is already
/// cached under `<gradle_user_home>/wrapper/dists/` — used only to print a
/// one-time note that the first run downloads Gradle (spec §12.4 step 4),
/// never to gate the build.
pub fn wrapper_dist_cached(gradle_user_home: &Path) -> bool {
    gradle_user_home
        .join("wrapper")
        .join("dists")
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn assemble_debug_runs_gradlew_with_java_home_env_and_prefixes_lines() {
        let runner = FakeProcessRunner::new().with(
            "./gradlew assembleDebug",
            Output {
                success: true,
                stdout: "> Task :app:assembleDebug\nBUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );
        let mut lines = Vec::new();
        let out = assemble_debug(
            &runner,
            Path::new("/tmp/myapp/android"),
            "/opt/jdk17",
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert!(out.success);
        assert_eq!(
            lines,
            vec![
                "[gradle] > Task :app:assembleDebug",
                "[gradle] BUILD SUCCESSFUL",
            ]
        );
    }

    #[test]
    fn assemble_debug_surfaces_failure() {
        let runner = FakeProcessRunner::new().with(
            "./gradlew assembleDebug",
            Output {
                success: false,
                stdout: "> Task :app:compileDebugKotlin FAILED".to_string(),
                stderr: "e: compile error".to_string(),
            },
        );
        let out = assemble_debug(&runner, Path::new("android"), "/opt/jdk17", &mut |_| {}).unwrap();
        assert!(!out.success);
    }

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "forgekit-cli-gradle-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn wrapper_dist_cached_false_when_dists_dir_absent() {
        let dir = unique_temp_dir("no-dists");
        fs::create_dir_all(&dir).unwrap();
        assert!(!wrapper_dist_cached(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrapper_dist_cached_true_when_dists_populated() {
        let dir = unique_temp_dir("with-dists");
        let dists = dir.join("wrapper").join("dists").join("gradle-9.5-bin");
        fs::create_dir_all(&dists).unwrap();
        fs::write(dists.join("marker"), b"x").unwrap();
        assert!(wrapper_dist_cached(&dir));
        let _ = fs::remove_dir_all(&dir);
    }
}
