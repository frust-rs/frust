//! The desktop `cargo run` launch plan shared by any front-end that needs to
//! preview a Frust project on the host rather than a device — the same shape
//! `frust-tui`'s supervisor already builds for its own desktop sessions
//! (`crates/frust-tui/src/supervise/session.rs`'s `SessionSpec::desktop_plan`),
//! pulled down here so a second front-end (e.g. an MCP server driving a Frust
//! app for inspection) can spawn the identical invocation without
//! reimplementing the mode → cargo-args mapping. Pure value-building plus a
//! thin [`ProcessRunner`] wrapper — no threads, no tokio, matching this
//! crate's charter (`docs/CLI_ARCHITECTURE.md`).
//!
//! `frust-tui` is not converged onto this module yet — that's a deferred
//! followup, not a regression; this module's job for now is behavior parity
//! (same features, same profile flags per [`BuildMode`]), verified by the
//! tests below.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::build_info::BuildInfo;
use crate::process::{ProcessRunner, StreamHandle};

/// The resolved `cargo run` desktop-preview invocation for a [`BuildInfo`] in
/// `root`: which program to spawn, its arguments in order, and the working
/// directory to spawn it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopPlan {
    /// The program to spawn — always `cargo`.
    pub program: String,
    /// Its arguments, in order (e.g. `["run", "--features", "frust/perf-trace", ...]`).
    pub args: Vec<String>,
    /// The child's working directory — the project root.
    pub cwd: PathBuf,
}

/// Builds the `cargo run` plan for `info` in `root`: the mode's cargo
/// profile flag(s) (`BuildMode::cargo_profile_arg`) followed by one
/// `--features <name>` pair per mode-selected cargo feature
/// (`BuildMode::cargo_features` — `frust/perf-trace`+`frust/devtools` for
/// Debug/Profile, `lean` for Release). Without the feature flags the spawned
/// preview compiles the devtools service and perf instrumentation OUT, so a
/// tool waiting on a discovery line would wait forever.
///
/// Pure: no I/O, no process spawn — see [`spawn_desktop_session`] for the
/// side-effecting wrapper.
pub fn desktop_plan(root: &Path, info: &BuildInfo) -> DesktopPlan {
    let mut args = vec!["run".to_string()];
    for arg in info.mode.cargo_profile_arg() {
        args.push((*arg).to_string());
    }
    for feature in info.mode.cargo_features() {
        args.push("--features".to_string());
        args.push((*feature).to_string());
    }

    DesktopPlan {
        program: "cargo".to_string(),
        args,
        cwd: root.to_path_buf(),
    }
}

/// Resolves [`desktop_plan`] for `info` in `root` and spawns it through
/// `runner` (`docs/CODE_STANDARDS.md`'s "shelling out through `ProcessRunner`"
/// contract — never `std::process` directly), handing back the caller's
/// [`StreamHandle`] over the child's merged stdout/stderr.
pub fn spawn_desktop_session(
    runner: &dyn ProcessRunner,
    root: &Path,
    info: &BuildInfo,
) -> Result<StreamHandle> {
    let plan = desktop_plan(root, info);
    let args: Vec<&str> = plan.args.iter().map(String::as_str).collect();
    runner
        .spawn_streaming(&plan.program, &args, Some(plan.cwd.as_path()), &[])
        .with_context(|| format!("spawning `{} {}`", plan.program, plan.args.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::BuildMode;
    use crate::process::FakeProcessRunner;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn build_info(mode: BuildMode) -> BuildInfo {
        BuildInfo {
            mode,
            flavor: None,
            defines: HashMap::new(),
            build_name: None,
            build_number: None,
        }
    }

    #[test]
    fn debug_plan_matches_the_tui_desktop_plan() {
        let root = PathBuf::from("/tmp/project");
        let plan = desktop_plan(&root, &build_info(BuildMode::Debug));
        assert_eq!(plan.program, "cargo");
        assert_eq!(
            plan.args,
            vec![
                "run",
                "--features",
                "frust/perf-trace",
                "--features",
                "frust/devtools",
            ]
        );
        assert_eq!(plan.cwd, root);
    }

    #[test]
    fn profile_plan_matches_the_tui_desktop_plan() {
        let root = PathBuf::from("/tmp/project");
        let plan = desktop_plan(&root, &build_info(BuildMode::Profile));
        assert_eq!(
            plan.args,
            vec![
                "run",
                "--profile",
                "profile",
                "--features",
                "frust/perf-trace",
                "--features",
                "frust/devtools",
            ]
        );
        assert_eq!(plan.cwd, root);
    }

    #[test]
    fn release_plan_matches_the_tui_desktop_plan() {
        let root = PathBuf::from("/tmp/project");
        let plan = desktop_plan(&root, &build_info(BuildMode::Release));
        assert_eq!(plan.args, vec!["run", "--release", "--features", "lean"]);
        assert_eq!(plan.cwd, root);
    }

    #[test]
    fn spawn_desktop_session_goes_through_the_injected_runner() {
        let root = PathBuf::from("/tmp/project");
        let runner = FakeProcessRunner::new().with_stream(
            "cargo run --release --features lean",
            ["Compiling frust v0.1.0", "Running `target/release/app`"],
            true,
        );
        let mut handle =
            spawn_desktop_session(&runner, &root, &build_info(BuildMode::Release)).unwrap();
        let lines: Vec<String> = handle.lines.iter().collect();
        assert_eq!(
            lines,
            vec!["Compiling frust v0.1.0", "Running `target/release/app`"]
        );
        assert!(handle.wait());
    }

    #[test]
    fn spawn_desktop_session_reports_the_project_root_as_cwd() {
        let root = PathBuf::from("/tmp/project");
        let runner = FakeProcessRunner::new().with_stream(
            "cargo run --features frust/perf-trace --features frust/devtools",
            Vec::<&str>::new(),
            true,
        );
        let mut handle =
            spawn_desktop_session(&runner, &root, &build_info(BuildMode::Debug)).unwrap();
        handle.wait();
        assert_eq!(runner.recorded_cwd(), Some(root));
    }
}
