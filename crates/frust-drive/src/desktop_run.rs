//! The desktop `cargo run` launch plan — the workspace's **single**
//! construction site for what a desktop preview of a Frust project actually
//! runs. Every front-end that previews on the host rather than a device
//! resolves its invocation here: `frust run`'s desktop fallback (which adds
//! the release-lean preflight's resolved feature list on top), `frust-tui`'s
//! supervisor
//! (`crates/frust-tui/src/supervise/session.rs`'s `SessionSpec::launch_plan`,
//! which reshapes the result into its own `LaunchPlan`), and `frust-mcp`'s
//! session engine (through [`spawn_desktop_session`]). Pure value-building plus
//! a thin [`ProcessRunner`] wrapper — no threads, no tokio, matching this
//! crate's charter (`docs/CLI_ARCHITECTURE.md`).
//!
//! A plan carries its own child environment as well as its argv, because the
//! two halves are one behavior: a `--profile` preview needs `FRUST_TRACE=1` set
//! for the same reason it needs `--features frust/perf-trace` compiled in, and
//! a front-end that resolved only the args would silently drop half of it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::build_info::{BuildInfo, BuildMode};
use crate::process::{ProcessRunner, StreamHandle};

/// The env var a `--profile` desktop preview needs set for the shell's perf
/// instrumentation to actually emit trace lines — the same variable
/// `BuildInfo::from_args` auto-injects as a define for a `--profile` build.
const TRACE_ENV_VAR: &str = "FRUST_TRACE";

/// The resolved `cargo run` desktop-preview invocation for a [`BuildInfo`] in
/// `root`: which program to spawn, its arguments in order, the working
/// directory to spawn it in, and the extra environment the child needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopPlan {
    /// The program to spawn — always `cargo`.
    pub program: String,
    /// Its arguments, in order (e.g. `["run", "--features", "frust/perf-trace", ...]`).
    pub args: Vec<String>,
    /// The child's working directory — the project root.
    pub cwd: PathBuf,
    /// Extra environment for the child only (added/overridden, never touching
    /// the parent's environment): every `--define KEY=VALUE` from `info`, plus
    /// the auto-injected [`TRACE_ENV_VAR`] for a profile build that doesn't
    /// already define it. Sorted by key, so a plan (and any test scripting it)
    /// is deterministic despite the defines living in a `HashMap`.
    pub env: Vec<(String, String)>,
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
/// side-effecting wrapper, and [`desktop_plan_with_features`] for the caller
/// that resolved its own feature list first.
pub fn desktop_plan(root: &Path, info: &BuildInfo) -> DesktopPlan {
    desktop_plan_with_features(root, info, info.mode.cargo_features())
}

/// [`desktop_plan`] over a caller-resolved `features` list rather than the
/// mode's own.
///
/// The one caller that needs this is `frust run`'s desktop fallback, which
/// resolves its list through
/// [`crate::cargo_manifest::resolve_release_features`] for two reasons: the
/// release-lean preflight drops `lean` for an app that predates the feature
/// (passing an undeclared `--features lean` to `cargo run` fails the whole
/// build with cargo's opaque message), and `--features` passthrough entries
/// are appended after the mode's own. Passing `info.mode.cargo_features()`
/// here is byte-identical to [`desktop_plan`].
pub fn desktop_plan_with_features(root: &Path, info: &BuildInfo, features: &[&str]) -> DesktopPlan {
    let mut args = vec!["run".to_string()];
    for arg in info.mode.cargo_profile_arg() {
        args.push((*arg).to_string());
    }
    for feature in features {
        args.push("--features".to_string());
        args.push((*feature).to_string());
    }

    DesktopPlan {
        program: "cargo".to_string(),
        args,
        cwd: root.to_path_buf(),
        env: desktop_env(info),
    }
}

/// The child environment a desktop preview of `info` runs with: every
/// `--define KEY=VALUE` as an env pair, plus [`TRACE_ENV_VAR`]`=1` for a
/// profile build that doesn't define it itself.
///
/// The injection exists because not every front-end funnels through
/// `BuildInfo::from_args` (which does the same thing as a *define*): the
/// workbench's run-config modal builds a [`BuildInfo`] by hand, so a
/// `--profile` desktop session would otherwise start with perf instrumentation
/// compiled in and nothing turning it on. An explicit user define always wins.
fn desktop_env(info: &BuildInfo) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = info
        .defines
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if info.mode == BuildMode::Profile && !env.iter().any(|(key, _)| key == TRACE_ENV_VAR) {
        env.push((TRACE_ENV_VAR.to_string(), "1".to_string()));
    }
    env.sort();
    env
}

/// Resolves [`desktop_plan`] for `info` in `root` and spawns it through
/// `runner` (`docs/CODE_STANDARDS.md`'s "shelling out through `ProcessRunner`"
/// contract — never `std::process` directly), handing back the caller's
/// [`StreamHandle`] over the child's merged stdout/stderr.
///
/// The plan's whole shape is spawned, environment included — a caller that got
/// the args but not the env would be running a different preview than the one
/// this module resolved.
pub fn spawn_desktop_session(
    runner: &dyn ProcessRunner,
    root: &Path,
    info: &BuildInfo,
) -> Result<StreamHandle> {
    let plan = desktop_plan(root, info);
    let args: Vec<&str> = plan.args.iter().map(String::as_str).collect();
    let env: Vec<(&str, &str)> = plan
        .env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    runner
        .spawn_streaming(&plan.program, &args, Some(plan.cwd.as_path()), &env)
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
    fn debug_plan_carries_perf_trace_and_devtools_features() {
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
        assert!(plan.env.is_empty());
    }

    #[test]
    fn profile_plan_threads_the_profile_flag() {
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
    fn release_plan_carries_lean() {
        let root = PathBuf::from("/tmp/project");
        let plan = desktop_plan(&root, &build_info(BuildMode::Release));
        assert_eq!(plan.args, vec!["run", "--release", "--features", "lean"]);
        assert_eq!(plan.cwd, root);
        assert!(plan.env.is_empty());
    }

    /// The release-lean preflight's direction: a legacy app resolves to an
    /// EMPTY feature list, and the plan then simply omits `--features` rather
    /// than passing cargo an undeclared feature.
    #[test]
    fn a_caller_resolved_feature_list_replaces_the_modes_own() {
        let root = PathBuf::from("/tmp/project");
        let info = build_info(BuildMode::Release);
        assert_eq!(
            desktop_plan_with_features(&root, &info, &[]).args,
            vec!["run", "--release"]
        );
        // The declaring direction is byte-identical to `desktop_plan`.
        assert_eq!(
            desktop_plan_with_features(&root, &info, &["lean"]).args,
            desktop_plan(&root, &info).args
        );
    }

    #[test]
    fn defines_become_sorted_child_env_pairs() {
        let mut info = build_info(BuildMode::Debug);
        info.defines.insert("Z_KEY".into(), "1".into());
        info.defines.insert("A_KEY".into(), "2".into());
        let plan = desktop_plan(Path::new("/tmp/project"), &info);
        assert_eq!(
            plan.env,
            vec![
                ("A_KEY".to_string(), "2".to_string()),
                ("Z_KEY".to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn a_profile_plan_injects_frust_trace_unless_the_caller_defined_it() {
        let plan = desktop_plan(Path::new("/tmp/project"), &build_info(BuildMode::Profile));
        assert_eq!(
            plan.env,
            vec![("FRUST_TRACE".to_string(), "1".to_string())],
            "a --profile desktop preview turns instrumentation on, not just in"
        );

        let mut info = build_info(BuildMode::Profile);
        info.defines.insert("FRUST_TRACE".into(), "0".into());
        assert_eq!(
            desktop_plan(Path::new("/tmp/project"), &info).env,
            vec![("FRUST_TRACE".to_string(), "0".to_string())],
            "an explicit user define always wins over the auto-inject"
        );
    }

    #[test]
    fn debug_and_release_plans_never_inject_frust_trace() {
        for mode in [BuildMode::Debug, BuildMode::Release] {
            let plan = desktop_plan(Path::new("/tmp/project"), &build_info(mode));
            assert!(plan.env.is_empty(), "{mode:?} must not inject FRUST_TRACE");
        }
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
