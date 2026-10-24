//! Cargo-resolve preflight for a Debug/Profile build: recognises the
//! stale-index failure the debug-only frust crates hit after a release,
//! refreshes cargo's local index entries for them, and retries once.
//!
//! `frust-hotpatch`, `frust-devtools` and `frust-devtools-protocol` enter an
//! app's dependency graph only through the features a Debug/Profile build
//! passes (`build_info::BuildMode::session_cargo_features`), so they are not
//! in the app's `Cargo.lock` until that first build. With a lockfile present,
//! cargo resolves a newly added crate from its local sparse-index cache
//! without refreshing it, so a cache written before a frust release fails with
//! `failed to select a version for the requirement`. `cargo update`,
//! `cargo generate-lockfile` and `cargo fetch` leave that cache entry stale;
//! `cargo add <crate>@<req> --dry-run` refreshes it and writes nothing.
//!
//! [`ensure_resolves`] never writes `Cargo.toml`, never touches files under
//! `CARGO_HOME` itself, and runs at most one refresh and one retry. A
//! path-dependency app resolves the crates from its checkout, so it takes the
//! first-try path with no special case.

use std::path::{Path, PathBuf};

use crate::process::{Output, ProcessRunner};

/// The frust crates only a Debug/Profile build's features pull in — the ones
/// an app's `Cargo.lock` lacks until its first such build.
pub const DEBUG_ONLY_CRATES: [&str; 3] = [
    "frust-hotpatch",
    "frust-devtools",
    "frust-devtools-protocol",
];

/// What [`ensure_resolves`] had to do for the dependency graph to resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightOutcome {
    /// The first `cargo metadata` resolve succeeded.
    Resolved,
    /// The first resolve hit a stale index entry for a frust crate, and the
    /// resolve after one `cargo add --dry-run` refresh succeeded.
    Refreshed,
}

/// Why the app's dependency graph does not resolve.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PreflightError {
    /// `cargo` could not be spawned at all.
    #[error(
        "could not run `cargo {command}` for '{}': {reason} — is cargo installed and on PATH?",
        app_root.display()
    )]
    CargoUnavailable {
        app_root: PathBuf,
        command: String,
        reason: String,
    },
    /// The resolve failed for a reason this preflight does not repair;
    /// `stderr` is cargo's own text, unchanged.
    #[error("{stderr}")]
    ResolveFailed { stderr: String },
    /// A frust crate's requirement is newer than cargo's local index cache,
    /// and the refresh was skipped (cargo is offline) or did not help.
    /// `hint` is the one-line manual remedy printed after cargo's `stderr`.
    #[error("{stderr}\n{hint}")]
    StaleIndex {
        crate_name: String,
        requirement: String,
        stderr: String,
        hint: String,
    },
}

/// Resolves the dependency graph of the app at `app_root` under
/// `cargo_features` (already spelled with the app's facade key, as
/// `build_info::BuildMode::session_cargo_features_for` returns them),
/// repairing a stale index entry for a frust crate once on the way.
/// `on_line` receives the single line announcing a refresh.
pub fn ensure_resolves(
    runner: &dyn ProcessRunner,
    app_root: &Path,
    cargo_features: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<PreflightOutcome, PreflightError> {
    let env_offline = std::env::var("CARGO_NET_OFFLINE").ok();
    ensure_resolves_with(
        runner,
        app_root,
        cargo_features,
        env_offline.as_deref(),
        on_line,
    )
}

/// [`ensure_resolves`] with the `CARGO_NET_OFFLINE` value passed in, so a
/// test never reads or sets the process environment.
fn ensure_resolves_with(
    runner: &dyn ProcessRunner,
    app_root: &Path,
    cargo_features: &[String],
    env_offline: Option<&str>,
    on_line: &mut dyn FnMut(&str),
) -> Result<PreflightOutcome, PreflightError> {
    let manifest = app_root.join("Cargo.toml");
    let metadata = metadata_args(&manifest, cargo_features);
    let first = run_cargo(runner, app_root, &metadata)?;
    if first.success {
        return Ok(PreflightOutcome::Resolved);
    }
    let Some((crate_name, requirement)) = frust_stale_requirement(&first.stderr) else {
        return Err(PreflightError::ResolveFailed {
            stderr: first.stderr,
        });
    };

    let crates = refresh_crates(&crate_name);
    let refresh = refresh_args(&manifest, &crates, &requirement);
    let stale = |crate_name: String, requirement: String, stderr: &str, offline: bool| {
        let hint = stale_hint(&crate_name, &requirement, &refresh, offline);
        PreflightError::StaleIndex {
            crate_name,
            requirement,
            stderr: stderr.trim_end().to_string(),
            hint,
        }
    };
    if is_offline(&metadata, env_offline) {
        return Err(stale(crate_name, requirement, &first.stderr, true));
    }

    let added = run_cargo(runner, app_root, &refresh)?;
    if !added.success {
        let stderr = format!(
            "{}\n`cargo add --dry-run` could not refresh the index entries:\n{}",
            first.stderr.trim_end(),
            added.stderr.trim_end()
        );
        return Err(stale(crate_name, requirement, &stderr, false));
    }
    on_line(&format!(
        "refreshed stale crates.io index entries for {} (cached before frust {})",
        crates.join(", "),
        release_version(&requirement)
    ));

    let retry = run_cargo(runner, app_root, &metadata)?;
    if retry.success {
        return Ok(PreflightOutcome::Refreshed);
    }
    // Name the crate the retry tripped on when it is a different frust crate.
    let (crate_name, requirement) =
        frust_stale_requirement(&retry.stderr).unwrap_or((crate_name, requirement));
    Err(stale(crate_name, requirement, &retry.stderr, false))
}

/// The full-graph resolve: no `--no-deps`, one `--features` per entry in the
/// caller's order.
fn metadata_args(manifest: &Path, cargo_features: &[String]) -> Vec<String> {
    let mut args = vec![
        "metadata".to_string(),
        "--format-version".to_string(),
        "1".to_string(),
        "--manifest-path".to_string(),
        manifest.to_string_lossy().into_owned(),
    ];
    for feature in cargo_features {
        args.push("--features".to_string());
        args.push(feature.clone());
    }
    args
}

/// The refresh: every crate in `crates` at `requirement` — the frust crates
/// release in lockstep, so the failing crate's requirement fits them all.
fn refresh_args(manifest: &Path, crates: &[String], requirement: &str) -> Vec<String> {
    let mut args = vec!["add".to_string()];
    args.extend(crates.iter().map(|name| format!("{name}@{requirement}")));
    args.push("--dry-run".to_string());
    args.push("--manifest-path".to_string());
    args.push(manifest.to_string_lossy().into_owned());
    args
}

/// [`DEBUG_ONLY_CRATES`], plus `failing` when it is not one of them.
fn refresh_crates(failing: &str) -> Vec<String> {
    let mut crates: Vec<String> = DEBUG_ONLY_CRATES.iter().map(|c| c.to_string()).collect();
    if !DEBUG_ONLY_CRATES.contains(&failing) {
        crates.push(failing.to_string());
    }
    crates
}

/// Runs `cargo <args>` in `app_root`, so cargo reads the app's own
/// `.cargo/config.toml` exactly as the build that follows does. Stdout (the
/// metadata document) is discarded.
fn run_cargo(
    runner: &dyn ProcessRunner,
    app_root: &Path,
    args: &[String],
) -> Result<Output, PreflightError> {
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    runner
        .run_streaming("cargo", &argv, Some(app_root), &[], &mut |_| {})
        .map_err(|err| PreflightError::CargoUnavailable {
            app_root: app_root.to_path_buf(),
            command: args.first().cloned().unwrap_or_default(),
            reason: format!("{err:#}"),
        })
}

/// Whether cargo runs offline: an `--offline` on the resolve's argv or a
/// truthy `CARGO_NET_OFFLINE`. A `net.offline` cargo config is not read here;
/// under it the refresh `cargo add` fails and the hint is returned all the same.
fn is_offline(args: &[String], env_offline: Option<&str>) -> bool {
    args.iter().any(|arg| arg == "--offline")
        || env_offline.is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "true" | "1" | "yes" | "on"
            )
        })
}

/// [`stale_requirement`], kept only when it names a frust crate.
fn frust_stale_requirement(stderr: &str) -> Option<(String, String)> {
    stale_requirement(stderr).filter(|(name, _)| name.starts_with("frust-"))
}

/// The `(crate, requirement)` of cargo's
/// ``failed to select a version for the requirement `<crate> = "<req>"` ``
/// line, matched on that stable prefix with any backtick, quote or whitespace
/// variant around the two values; `None` for every other error.
fn stale_requirement(stderr: &str) -> Option<(String, String)> {
    const PREFIX: &str = "failed to select a version for the requirement";
    let rest = stderr
        .lines()
        .find_map(|line| line.find(PREFIX).map(|at| &line[at + PREFIX.len()..]))?;
    let (name, requirement) = rest.split_once('=')?;
    let is_quote = |c: char| matches!(c, '`' | '"' | '\'');
    let name = name.trim().trim_matches(is_quote).trim();
    let requirement = requirement.trim_start().trim_start_matches(is_quote);
    let requirement = requirement[..requirement.find(is_quote).unwrap_or(requirement.len())].trim();
    let valid_name = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    (valid_name && !requirement.is_empty()).then(|| (name.to_string(), requirement.to_string()))
}

/// The release a requirement names: `^0.6.1` and `=0.6.1` read `0.6.1`.
fn release_version(requirement: &str) -> &str {
    let version = requirement
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .split([',', ' '])
        .next()
        .unwrap_or_default();
    if version.is_empty() {
        requirement
    } else {
        version
    }
}

/// The one-line manual remedy for a stale index entry: the exact refresh
/// command, or the template feature that keeps the crates in `Cargo.lock`.
fn stale_hint(crate_name: &str, requirement: &str, refresh: &[String], offline: bool) -> String {
    let command = refresh
        .iter()
        .map(|arg| {
            if arg.contains(char::is_whitespace) {
                format!("'{arg}'")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let skipped = if offline {
        " (cargo is offline, so it was not refreshed automatically)"
    } else {
        ""
    };
    format!(
        "hint: `{crate_name} = \"{requirement}\"` is newer than cargo's local crates.io index \
         cache, which predates the frust {} release{skipped}; refresh it with `cargo {command}` \
         (Cargo.toml stays unchanged), or add `frust-debug = [\"frust/perf-trace\", \
         \"frust/devtools\", \"frust/hotpatch\"]` to [features] in Cargo.toml",
        release_version(requirement)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::StreamHandle;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Replies to each streamed `cargo` call with the next scripted
    /// [`Output`], recording the argv and cwd of every call.
    struct ScriptedRunner {
        replies: Mutex<VecDeque<anyhow::Result<Output>>>,
        calls: Mutex<Vec<(Vec<String>, Option<PathBuf>)>>,
    }

    impl ScriptedRunner {
        fn new(replies: impl IntoIterator<Item = Output>) -> Self {
            Self::scripted(replies.into_iter().map(Ok))
        }

        fn scripted(replies: impl IntoIterator<Item = anyhow::Result<Output>>) -> Self {
            Self {
                replies: Mutex::new(replies.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn argvs(&self) -> Vec<Vec<String>> {
            let calls = self.calls.lock().unwrap();
            calls.iter().map(|(argv, _)| argv.clone()).collect()
        }

        fn count(&self, subcommand: &str) -> usize {
            let argvs = self.argvs();
            argvs.iter().filter(|argv| argv[1] == subcommand).count()
        }
    }

    impl ProcessRunner for ScriptedRunner {
        fn run(&self, cmd: &str, _args: &[&str]) -> anyhow::Result<Output> {
            anyhow::bail!("the preflight streams every call; unexpected `{cmd}`")
        }

        fn run_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            _env: &[(&str, &str)],
            _on_line: &mut dyn FnMut(&str),
        ) -> anyhow::Result<Output> {
            let argv = std::iter::once(cmd)
                .chain(args.iter().copied())
                .map(str::to_string)
                .collect();
            let mut calls = self.calls.lock().unwrap();
            calls.push((argv, cwd.map(Path::to_path_buf)));
            let mut replies = self.replies.lock().unwrap();
            replies
                .pop_front()
                .unwrap_or_else(|| anyhow::bail!("unscripted `{cmd} {}`", args.join(" ")))
        }

        fn spawn_streaming(
            &self,
            cmd: &str,
            _args: &[&str],
            _cwd: Option<&Path>,
            _env: &[(&str, &str)],
        ) -> anyhow::Result<StreamHandle> {
            anyhow::bail!("ScriptedRunner does not spawn `{cmd}`")
        }
    }

    fn ok() -> Output {
        Output {
            success: true,
            stdout: "{\"packages\":[]}".to_string(),
            stderr: String::new(),
        }
    }

    fn failed(stderr: &str) -> Output {
        Output {
            success: false,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    /// Cargo's stale-index failure in its real shape.
    fn stale_stderr(crate_name: &str, requirement: &str, required_by: &str) -> String {
        format!(
            "    Updating crates.io index\n\
             error: failed to select a version for the requirement `{crate_name} = \"{requirement}\"`\n\
             candidate versions found which didn't match: 0.6.0\n\
             location searched: crates.io index\n\
             required by package `{required_by} v0.6.1`\n    \
             ... which satisfies dependency `frust-core = \"^0.6.1\"` of package `frust-ui v0.6.1`\n\
             perhaps a crate was updated and forgotten to be re-generated?\n"
        )
    }

    fn app_root() -> PathBuf {
        PathBuf::from("/work/app")
    }

    fn manifest() -> String {
        app_root().join("Cargo.toml").to_string_lossy().into_owned()
    }

    fn features() -> Vec<String> {
        vec!["app/perf-trace".to_string(), "app/devtools".to_string()]
    }

    fn preflight(
        runner: &ScriptedRunner,
        env_offline: Option<&str>,
    ) -> (Result<PreflightOutcome, PreflightError>, Vec<String>) {
        let mut lines = Vec::new();
        let result =
            ensure_resolves_with(runner, &app_root(), &features(), env_offline, &mut |line| {
                lines.push(line.to_string())
            });
        (result, lines)
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn parser_reads_the_requirement_for_each_debug_only_crate() {
        let required_by = ["frust-core", "frust-core", "frust-devtools"];
        for (name, by) in DEBUG_ONLY_CRATES.iter().zip(required_by) {
            assert_eq!(
                stale_requirement(&stale_stderr(name, "^0.6.1", by)),
                Some((name.to_string(), "^0.6.1".to_string())),
                "{name}"
            );
        }
    }

    #[test]
    fn parser_tolerates_quote_and_whitespace_variants() {
        for line in [
            "error: failed to select a version for the requirement `frust-devtools = \"=0.6.1\"`",
            "failed to select a version for the requirement frust-devtools=\"=0.6.1\"",
            "failed to select a version for the requirement   'frust-devtools'  =  '=0.6.1'  ",
            "failed to select a version for the requirement `frust-devtools = =0.6.1`\r",
        ] {
            assert_eq!(
                stale_requirement(line),
                Some(("frust-devtools".to_string(), "=0.6.1".to_string())),
                "{line}"
            );
        }
    }

    #[test]
    fn parser_ignores_unrelated_errors() {
        for stderr in [
            "error: failed to select a version for `serde`.\n    ... required by package `app v0.1.0`\n\
             versions that meet the requirements `^1.0` are: 1.0.228",
            "error: no matching package named `frust-hotpatch` found\nlocation searched: crates.io index",
            "error: failed to select a version for the requirement ``",
            "",
        ] {
            assert_eq!(stale_requirement(stderr), None, "{stderr}");
        }
    }

    #[test]
    fn first_try_success_runs_one_metadata_call_in_feature_order() {
        let runner = ScriptedRunner::new([ok()]);
        let (result, lines) = preflight(&runner, None);

        assert_eq!(result, Ok(PreflightOutcome::Resolved));
        assert!(lines.is_empty());
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (argv, cwd) = &calls[0];
        let mut expected = strings(&["cargo", "metadata", "--format-version", "1"]);
        expected.extend(["--manifest-path".to_string(), manifest()]);
        expected.extend(strings(&[
            "--features",
            "app/perf-trace",
            "--features",
            "app/devtools",
        ]));
        assert_eq!(argv, &expected);
        assert_eq!(cwd.as_deref(), Some(app_root().as_path()));
    }

    #[test]
    fn refresh_names_every_debug_only_crate_at_the_failing_requirement() {
        let stale = stale_stderr("frust-hotpatch", "^0.6.1", "frust-core");
        let runner = ScriptedRunner::new([failed(&stale), ok(), ok()]);
        let (result, lines) = preflight(&runner, None);

        assert_eq!(result, Ok(PreflightOutcome::Refreshed));
        let mut expected = strings(&[
            "cargo",
            "add",
            "frust-hotpatch@^0.6.1",
            "frust-devtools@^0.6.1",
            "frust-devtools-protocol@^0.6.1",
            "--dry-run",
            "--manifest-path",
        ]);
        expected.push(manifest());
        assert_eq!(runner.argvs()[1], expected);
        assert_eq!(runner.argvs()[2], runner.argvs()[0]);
        assert_eq!(
            lines,
            [
                "refreshed stale crates.io index entries for frust-hotpatch, frust-devtools, \
              frust-devtools-protocol (cached before frust 0.6.1)"
            ]
        );
    }

    #[test]
    fn refresh_adds_a_failing_frust_crate_outside_the_debug_only_set() {
        let stale = stale_stderr("frust-core", "^0.6.1", "frust-ui");
        let runner = ScriptedRunner::new([failed(&stale), ok(), ok()]);
        let (result, lines) = preflight(&runner, None);

        assert_eq!(result, Ok(PreflightOutcome::Refreshed));
        assert_eq!(runner.argvs()[1][5], "frust-core@^0.6.1");
        assert!(lines[0].contains("frust-devtools-protocol, frust-core (cached"));
    }

    #[test]
    fn failure_after_refresh_returns_the_hint_after_two_metadata_runs_and_one_add() {
        let stale = stale_stderr("frust-devtools-protocol", "^0.6.1", "frust-devtools");
        let runner = ScriptedRunner::new([failed(&stale), ok(), failed(&stale)]);
        let (result, _) = preflight(&runner, None);

        assert_eq!(runner.count("metadata"), 2);
        assert_eq!(runner.count("add"), 1);
        let Err(PreflightError::StaleIndex {
            crate_name, hint, ..
        }) = &result
        else {
            panic!("expected a stale-index error, got {result:?}");
        };
        assert_eq!(crate_name, "frust-devtools-protocol");
        assert!(!hint.contains('\n'), "{hint}");
        assert!(hint.contains("predates the frust 0.6.1 release"), "{hint}");
        assert!(
            hint.contains(&format!(
                "`cargo add frust-hotpatch@^0.6.1 frust-devtools@^0.6.1 \
                 frust-devtools-protocol@^0.6.1 --dry-run --manifest-path {}`",
                manifest()
            )),
            "{hint}"
        );
        assert!(
            hint.contains("frust-debug = [\"frust/perf-trace\""),
            "{hint}"
        );
        let message = result.unwrap_err().to_string();
        assert!(message.starts_with(stale.trim_end()), "{message}");
        assert!(
            message.ends_with("to [features] in Cargo.toml"),
            "{message}"
        );
    }

    #[test]
    fn a_failed_refresh_is_not_followed_by_a_retry() {
        let stale = stale_stderr("frust-hotpatch", "^0.6.1", "frust-core");
        let network = "error: failed to fetch `https://index.crates.io/config.json`";
        let runner = ScriptedRunner::new([failed(&stale), failed(network)]);
        let (result, lines) = preflight(&runner, None);

        assert_eq!(runner.argvs().len(), 2);
        assert!(lines.is_empty());
        let message = result.unwrap_err().to_string();
        assert!(message.contains(network), "{message}");
        assert!(message.contains("hint: `frust-hotpatch"), "{message}");
    }

    #[test]
    fn offline_cargo_skips_the_refresh_and_returns_the_hint() {
        let stale = stale_stderr("frust-hotpatch", "^0.6.1", "frust-core");
        let runner = ScriptedRunner::new([failed(&stale)]);
        let (result, lines) = preflight(&runner, Some("true"));

        assert_eq!(runner.argvs().len(), 1);
        assert!(lines.is_empty());
        let Err(PreflightError::StaleIndex { hint, .. }) = result else {
            panic!("expected a stale-index error");
        };
        assert!(hint.contains("cargo is offline"), "{hint}");
        assert!(hint.contains("cargo add frust-hotpatch@^0.6.1"), "{hint}");
    }

    #[test]
    fn offline_is_read_from_the_argv_and_a_truthy_env_value() {
        let plain = strings(&["metadata", "--format-version", "1"]);
        let flagged = strings(&["metadata", "--offline"]);
        assert!(!is_offline(&plain, None));
        assert!(is_offline(&flagged, None));
        for value in ["true", "TRUE", "1", " yes ", "on"] {
            assert!(is_offline(&plain, Some(value)), "{value}");
        }
        for value in ["false", "0", ""] {
            assert!(!is_offline(&plain, Some(value)), "{value}");
        }
    }

    #[test]
    fn a_non_frust_crate_returns_cargo_stderr_unchanged_without_a_refresh() {
        let stale = stale_stderr("serde", "^1.0.300", "app");
        let runner = ScriptedRunner::new([failed(&stale)]);
        let (result, lines) = preflight(&runner, None);

        assert_eq!(runner.argvs().len(), 1);
        assert!(lines.is_empty());
        assert_eq!(result, Err(PreflightError::ResolveFailed { stderr: stale }));
    }

    #[test]
    fn a_cargo_spawn_failure_is_reported_as_unavailable() {
        let runner = ScriptedRunner::scripted([Err(anyhow::anyhow!("No such file"))]);
        let (result, _) = preflight(&runner, None);

        let Err(PreflightError::CargoUnavailable {
            command, reason, ..
        }) = result
        else {
            panic!("expected cargo to be unavailable");
        };
        assert_eq!(command, "metadata");
        assert!(reason.contains("No such file"), "{reason}");
    }

    #[test]
    fn release_version_strips_the_requirement_operator() {
        assert_eq!(release_version("^0.6.1"), "0.6.1");
        assert_eq!(release_version("=0.6.1"), "0.6.1");
        assert_eq!(release_version(">=0.6.1, <0.7"), "0.6.1");
        assert_eq!(release_version("*"), "*");
    }
}
