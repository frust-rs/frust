//! Print-free core of `frust clean`: runs `cargo clean` through the injected
//! [`ProcessRunner`], then removes [`build_dirs::CLEAN_DIRS`] and
//! [`build_dirs::LEGACY_CLEAN_DIRS`], reporting every step through an
//! `on_line` sink instead of printing — `frust-cli`'s `clean` command and
//! `frust-tui`'s clean session both call [`run`] and route its lines into
//! their own front end (stdout, a session log tab) instead of duplicating
//! this logic.
//!
//! Every entry is removed relative to a canonicalized `project_dir`: an
//! entry whose parent resolves (after following symlinks) outside that
//! canonical root is skipped rather than removed — guarding against an
//! intermediate path component that is itself a symlink escaping the
//! project — and an entry that is itself a symlink is unlinked rather than
//! recursively removed, so a symlinked `build/` never causes its target tree
//! to be deleted. See [`remove_path`].

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};

use crate::build_dirs::{
    CLEAN_DIRS, LEGACY_CLEAN_DIRS, LEGACY_WINDOWS_ICON, LEGACY_WINDOWS_ICON_MARKER,
};
use crate::process::ProcessRunner;

/// The outcome of a [`run`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanReport {
    /// `project_dir` has no `frust.toml` — nothing was cleaned. Mirrors the
    /// prior CLI/TUI "no `frust.toml` found ... — nothing to clean." refusal,
    /// which is also emitted through `on_line`.
    NotAFrustProject,
    /// `cargo clean` ran, followed by [`build_dirs::CLEAN_DIRS`]/
    /// [`build_dirs::LEGACY_CLEAN_DIRS`] removal.
    Cleaned {
        /// Whether `cargo clean` itself reported success
        /// ([`crate::process::Output::success`]) — a non-zero `cargo clean`
        /// exit does not stop the directory removal that follows it, the
        /// same tolerant behavior the CLI/TUI implementations this unifies
        /// both had.
        ///
        /// A caller surfaces `false` as failure rather than discarding it:
        /// `frust-cli`'s `commands::clean::run_in` maps it to exit code 1
        /// (`true`/[`CleanReport::NotAFrustProject`] both map to 0), and
        /// `frust-tui`'s `runner::run_clean` turns it into an `Err` so
        /// `launch_clean_session` lands the session at
        /// `SessionState::Exited(false)` instead of `Exited(true)`.
        cargo_clean_succeeded: bool,
    },
}

/// Runs `cargo clean` in `project_dir`, then removes
/// [`build_dirs::CLEAN_DIRS`] and [`build_dirs::LEGACY_CLEAN_DIRS`] under it,
/// reporting each step through `on_line`.
///
/// Refuses with [`CleanReport::NotAFrustProject`] — never an `Err` — when
/// `project_dir` has no `frust.toml`: there is nothing Frust generated there
/// to clean.
///
/// Runs `cargo clean` through [`ProcessRunner::run_streaming`] with `cwd` set
/// to `project_dir` explicitly — a bare [`ProcessRunner::run`] has no `cwd`
/// parameter and would run against the calling process's own working
/// directory instead of `project_dir` if the two ever differ (a front end
/// invoked from elsewhere than the project root). The streaming path's lines
/// reach `on_line` as they arrive.
///
/// Every entry in [`build_dirs::CLEAN_DIRS`] and
/// [`build_dirs::LEGACY_CLEAN_DIRS`] — plus [`build_dirs::LEGACY_WINDOWS_ICON`]
/// and its [`build_dirs::LEGACY_WINDOWS_ICON_MARKER`] when the marker says
/// Frust mirrored the icon there — is removed whether it names a directory
/// or a file; a missing entry is not an error, reported through `on_line`
/// with the wording `` Removed `<path>`. `` on success. See [`remove_path`]
/// for the symlink/containment semantics an entry can also hit. A removal
/// failure other than "not found" (permissions, a path that changed kind
/// under us, ...) is returned as an `Err` with context naming the path.
pub fn run(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    on_line: &mut dyn FnMut(&str),
) -> Result<CleanReport> {
    if !project_dir.join("frust.toml").exists() {
        on_line(&format!(
            "no `frust.toml` found in `{}` — nothing to clean.",
            project_dir.display()
        ));
        return Ok(CleanReport::NotAFrustProject);
    }

    let out = runner
        .run_streaming("cargo", &["clean"], Some(project_dir), &[], on_line)
        .context("failed to run `cargo clean`")?;
    if out.success {
        on_line("Removed cargo build artifacts (`cargo clean`).");
    } else {
        on_line(&format!("`cargo clean` failed:\n{}", out.stderr.trim()));
    }

    // Canonicalized once so every entry's containment check resolves
    // symlinks against the same root rather than re-walking it per entry.
    let canonical_project_dir = project_dir
        .canonicalize()
        .with_context(|| format!("resolving `{}`", project_dir.display()))?;
    for rel in CLEAN_DIRS.iter().chain(LEGACY_CLEAN_DIRS) {
        remove_path(&canonical_project_dir, project_dir, rel, on_line)?;
    }
    // The root-level `windows/icon.ico` is Frust output only when the
    // desktop pipeline mirrored it there for a pre-`build/`-layout
    // `windows/build.rs`, which it records with a sidecar marker; a
    // marker-less icon is the project owner's and is never touched.
    if project_dir.join(LEGACY_WINDOWS_ICON_MARKER).is_file() {
        remove_path(
            &canonical_project_dir,
            project_dir,
            LEGACY_WINDOWS_ICON,
            on_line,
        )?;
        remove_path(
            &canonical_project_dir,
            project_dir,
            LEGACY_WINDOWS_ICON_MARKER,
            on_line,
        )?;
    }

    Ok(CleanReport::Cleaned {
        cargo_clean_succeeded: out.success,
    })
}

/// Removes `project_dir.join(rel)`, tolerating "not found" and reporting a
/// successful removal through `on_line`. Any other I/O error (permissions,
/// ...) is returned with context.
///
/// Containment: `rel`'s *parent* directory is canonicalized and must resolve
/// inside `canonical_project_dir` (the caller's already-canonicalized
/// `project_dir`) — an intermediate path component that is itself a symlink
/// escaping the project resolves the parent elsewhere, and the entry is
/// skipped entirely (`` Skipped `<path>`: resolves outside the project. ``)
/// rather than walked into.
///
/// Kind: an entry that is itself a symlink (`symlink_metadata` reports
/// [`std::fs::Metadata::is_symlink`]) is unlinked, never recursively
/// removed — `fs::remove_file` on Unix (a symlink, even to a directory, is
/// never a directory to `symlink_metadata`), `fs::remove_dir` on Windows for
/// a directory reparse point (which `symlink_metadata`'s own `is_dir` does
/// report correctly there) — reported as `` Unlinked `<path>` (symlink). ``
/// rather than `` Removed ``, so a symlinked `build/` is detached without
/// deleting whatever tree it points at. A non-symlink directory or file is
/// removed and reported exactly as before (`fs::remove_dir_all`/
/// `fs::remove_file`, `` Removed `<path>`. ``).
fn remove_path(
    canonical_project_dir: &Path,
    project_dir: &Path,
    rel: &str,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    let path = project_dir.join(rel);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("reading `{}`", path.display())),
    };

    let parent = path.parent().unwrap_or(&path);
    let canonical_parent = match parent.canonicalize() {
        // The parent vanished between the `symlink_metadata` above and this
        // call (a concurrent removal) — treat the entry as already gone.
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        other => other.with_context(|| format!("resolving `{}`", parent.display()))?,
    };
    if !canonical_parent.starts_with(canonical_project_dir) {
        on_line(&format!(
            "Skipped `{}`: resolves outside the project.",
            path.display()
        ));
        return Ok(());
    }

    if metadata.is_symlink() {
        let result = if metadata.is_dir() {
            fs::remove_dir(&path)
        } else {
            fs::remove_file(&path)
        };
        return match result {
            Ok(()) => {
                on_line(&format!("Unlinked `{}` (symlink).", path.display()));
                Ok(())
            }
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err).with_context(|| format!("removing `{}`", path.display())),
        };
    }

    let result = if metadata.is_dir() {
        fs::remove_dir_all(&path)
    } else {
        fs::remove_file(&path)
    };
    match result {
        Ok(()) => {
            on_line(&format!("Removed `{}`.", path.display()));
            Ok(())
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("removing `{}`", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_project_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-clean-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_manifest(dir: &Path) {
        fs::write(dir.join("frust.toml"), "[app]\nname = \"x\"\norg = \"y\"\n").unwrap();
    }

    fn ok() -> Output {
        Output {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn noop_sink(_: &str) {}

    #[test]
    fn no_op_with_message_outside_a_frust_project() {
        let dir = unique_project_dir("no-toml");
        let runner = FakeProcessRunner::new(); // no `cargo clean` fixture registered
        let mut lines = Vec::new();
        let report = run(&runner, &dir, &mut |line| lines.push(line.to_string())).unwrap();
        assert_eq!(report, CleanReport::NotAFrustProject);
        assert!(lines.iter().any(|l| l.contains("nothing to clean")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn runs_cargo_clean_through_process_runner() {
        let dir = unique_project_dir("cargo-clean");
        write_manifest(&dir);
        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let report = run(&runner, &dir, &mut noop_sink).unwrap();
        assert_eq!(
            report,
            CleanReport::Cleaned {
                cargo_clean_succeeded: true
            }
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Regression for the clean-runs-in-the-wrong-directory bug: `run` must
    /// route `cargo clean` through `project_dir`, not the caller process's
    /// own `cwd` — asserted via `FakeProcessRunner::recorded_cwd`. Ports the
    /// CLI's and TUI's identically-named/-shaped regression tests.
    #[test]
    fn runs_cargo_clean_in_the_project_dir_not_the_process_cwd() {
        let dir = unique_project_dir("cargo-clean-cwd");
        write_manifest(&dir);
        // Sanity: the fixture directory is not the process's own cwd — proves
        // a bare (cwd-agnostic) `run` couldn't have hit this directory.
        assert_ne!(dir, std::env::current_dir().unwrap());

        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let report = run(&runner, &dir, &mut noop_sink).unwrap();
        assert_eq!(
            report,
            CleanReport::Cleaned {
                cargo_clean_succeeded: true
            }
        );
        assert_eq!(runner.recorded_cwd(), Some(dir.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn removes_the_build_dir() {
        let dir = unique_project_dir("removes-build-dir");
        write_manifest(&dir);
        fs::create_dir_all(dir.join("build/rust")).unwrap();
        fs::create_dir_all(dir.join("build/android/app")).unwrap();
        fs::write(dir.join("build/android/app/marker"), "x").unwrap();

        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let mut lines = Vec::new();
        run(&runner, &dir, &mut |line| lines.push(line.to_string())).unwrap();

        assert!(!dir.join("build").exists());
        assert!(
            lines
                .iter()
                .any(|l| l == &format!("Removed `{}`.", dir.join("build").display()))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn removes_legacy_directories() {
        let dir = unique_project_dir("removes-legacy-dirs");
        write_manifest(&dir);
        fs::create_dir_all(dir.join("android/app/build")).unwrap();
        fs::write(dir.join("android/app/build/marker"), "x").unwrap();
        // The `:frust-embedding` module's redirected Gradle output.
        fs::create_dir_all(dir.join("android/build/frust-embedding")).unwrap();
        fs::create_dir_all(dir.join("android/.gradle")).unwrap();
        fs::create_dir_all(dir.join("android/app/src/main/jniLibs")).unwrap();
        fs::create_dir_all(dir.join("dist/linux/my_app")).unwrap();

        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        run(&runner, &dir, &mut noop_sink).unwrap();

        assert!(!dir.join("android/app/build").exists());
        assert!(!dir.join("android/build").exists());
        assert!(!dir.join("android/.gradle").exists());
        assert!(!dir.join("android/app/src/main/jniLibs").exists());
        assert!(!dir.join("dist").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// `windows/icon.ico` at the project root without the mirror marker is
    /// the project owner's hand-placed input (b2-04 moved the *generated*
    /// icon under `build/desktop/windows/`), so `clean` must leave it alone.
    #[test]
    fn a_hand_placed_windows_icon_is_not_touched_by_clean() {
        let dir = unique_project_dir("hand-placed-icon");
        write_manifest(&dir);
        fs::create_dir_all(dir.join("windows")).unwrap();
        fs::write(dir.join("windows/icon.ico"), [0u8; 4]).unwrap();

        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        run(&runner, &dir, &mut noop_sink).unwrap();

        assert!(dir.join("windows/icon.ico").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The desktop pipeline's one-release mirror of the icon for a
    /// pre-`build/`-layout `windows/build.rs` writes `windows/icon.ico` next
    /// to the marker sidecar — that pair IS Frust output and `clean` removes
    /// both, reporting each, so no generated file survives outside `build/`.
    #[test]
    fn a_frust_mirrored_windows_icon_and_its_marker_are_removed_by_clean() {
        let dir = unique_project_dir("mirrored-icon");
        write_manifest(&dir);
        fs::create_dir_all(dir.join("windows")).unwrap();
        fs::write(dir.join(LEGACY_WINDOWS_ICON), [0u8; 4]).unwrap();
        fs::write(
            dir.join(LEGACY_WINDOWS_ICON_MARKER),
            "written by frust build\n",
        )
        .unwrap();
        fs::write(dir.join("windows/build.rs"), "fn main() {}\n").unwrap();

        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let mut lines = Vec::new();
        run(&runner, &dir, &mut |l| lines.push(l.to_string())).unwrap();

        assert!(!dir.join(LEGACY_WINDOWS_ICON).exists());
        assert!(!dir.join(LEGACY_WINDOWS_ICON_MARKER).exists());
        assert!(
            dir.join("windows/build.rs").exists(),
            "project sources must survive"
        );
        assert!(
            lines
                .iter()
                .any(|l| l == &format!("Removed `{}`.", dir.join(LEGACY_WINDOWS_ICON).display())),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l
                == &format!(
                    "Removed `{}`.",
                    dir.join(LEGACY_WINDOWS_ICON_MARKER).display()
                )),
            "{lines:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// `remove_path`'s plain-file removal branch, exercised directly since
    /// no production `CLEAN_DIRS`/`LEGACY_CLEAN_DIRS` entry is a bare file
    /// anymore.
    #[test]
    fn regular_file_entries_are_removed_and_reported() {
        let dir = unique_project_dir("regular-file-entry");
        write_manifest(&dir);
        fs::create_dir_all(dir.join("windows")).unwrap();
        fs::write(dir.join("windows/icon.ico"), [0u8; 4]).unwrap();

        let canonical = dir.canonicalize().unwrap();
        let mut lines = Vec::new();
        remove_path(&canonical, &dir, "windows/icon.ico", &mut |l| {
            lines.push(l.to_string())
        })
        .unwrap();

        assert!(!dir.join("windows/icon.ico").exists());
        // The containing directory itself is untouched — only the file was
        // targeted.
        assert!(dir.join("windows").exists());
        assert!(
            lines
                .iter()
                .any(|l| l == &format!("Removed `{}`.", dir.join("windows/icon.ico").display()))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A symlinked `build/` is unlinked, not traversed — the target tree it
    /// points at is untouched.
    #[test]
    #[cfg(unix)]
    fn symlinked_build_dir_is_unlinked_not_traversed() {
        let dir = unique_project_dir("symlinked-build");
        write_manifest(&dir);
        let target = unique_project_dir("symlinked-build-target");
        fs::write(target.join("marker"), "x").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("build")).unwrap();

        let canonical = dir.canonicalize().unwrap();
        let mut lines = Vec::new();
        remove_path(&canonical, &dir, "build", &mut |l| {
            lines.push(l.to_string())
        })
        .unwrap();

        // The symlink itself is gone; its target tree is untouched.
        assert!(!dir.join("build").exists());
        assert!(target.join("marker").exists());
        assert!(
            lines
                .iter()
                .any(|l| l == &format!("Unlinked `{}` (symlink).", dir.join("build").display()))
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&target);
    }

    /// An intermediate path component that is itself a symlink escaping the
    /// project is skipped, not walked into — nothing outside the project is
    /// deleted.
    #[test]
    #[cfg(unix)]
    fn intermediate_symlink_component_is_skipped() {
        let dir = unique_project_dir("intermediate-symlink");
        write_manifest(&dir);
        let outside = unique_project_dir("intermediate-symlink-outside");
        fs::create_dir_all(outside.join("app_target/build")).unwrap();
        fs::write(outside.join("app_target/build/marker"), "x").unwrap();
        fs::create_dir_all(dir.join("android")).unwrap();
        std::os::unix::fs::symlink(outside.join("app_target"), dir.join("android/app")).unwrap();

        let canonical = dir.canonicalize().unwrap();
        let mut lines = Vec::new();
        remove_path(&canonical, &dir, "android/app/build", &mut |l| {
            lines.push(l.to_string())
        })
        .unwrap();

        assert!(outside.join("app_target/build/marker").exists());
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Skipped") && l.contains("resolves outside the project"))
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn missing_dirs_are_not_an_error() {
        let dir = unique_project_dir("nothing-to-remove");
        write_manifest(&dir);
        let runner = FakeProcessRunner::new().with("cargo clean", ok());
        let report = run(&runner, &dir, &mut noop_sink).unwrap();
        assert_eq!(
            report,
            CleanReport::Cleaned {
                cargo_clean_succeeded: true
            }
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A failed `cargo clean` does not stop directory removal, and the
    /// report reflects the failure — `cargo_clean_succeeded: false`.
    #[test]
    fn a_failed_cargo_clean_still_removes_dirs_and_reports_failure() {
        let dir = unique_project_dir("cargo-clean-failed");
        write_manifest(&dir);
        fs::create_dir_all(dir.join("build")).unwrap();
        let failed = Output {
            success: false,
            stdout: String::new(),
            stderr: "boom".to_string(),
        };
        let runner = FakeProcessRunner::new().with("cargo clean", failed);
        let mut lines = Vec::new();
        let report = run(&runner, &dir, &mut |line| lines.push(line.to_string())).unwrap();

        assert_eq!(
            report,
            CleanReport::Cleaned {
                cargo_clean_succeeded: false
            }
        );
        assert!(!dir.join("build").exists());
        assert!(lines.iter().any(|l| l.contains("`cargo clean` failed")));
        let _ = fs::remove_dir_all(&dir);
    }
}
