//! Workspace replay: recompile modified (package, target) units by running
//! their captured rustc invocations again.
//!
//! Each captured invocation runs unchanged, with every crate type it was
//! captured with, except that `-Clinker` is stripped (a replayed crate must
//! produce real outputs, not re-enter the no-link interception) and
//! `--json=artifacts` is forced when absent. Replay writes at the same
//! paths cargo originally wrote to, so anything that must read the fat
//! build's outputs (the base DWARF) reads them before the first replay.
//!
//! Outputs are read from rustc's artifact notifications
//! (`{"artifact": <path>, "emit": "link"}` on stderr), the parse
//! dioxus-cli 0.7.10 applies to its tip build. The rlib to link is the link
//! artifact ending in `.rlib`, whatever its name: dioxus-cli's
//! `-C extra-filename` path and `lib<crate>-*.rlib` glob both miss the
//! `lib<crate>.rlib` cargo writes for a `["cdylib", "staticlib", "rlib"]`
//! lib. A missing capture or an unparsable notification is
//! [`HotpatchError::BuilderUnsupported`]. See `docs/CLI_ARCHITECTURE.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::process::ProcessRunner;

use super::HotpatchError;
use super::capture::{CAPTURE_ENV, RecordKey, RustcRecord, TargetKind, WORKSPACE_WRAPPER_ENV};
use super::graph::{ReplayUnit, WorkspaceGraph};
use super::link_intercept::{ENV_ARGS_FILE, ENV_ERR_FILE, ENV_LINK, ENV_LINKER};

/// Variables never passed to a replayed rustc: the wrappers (a replay must
/// not record or intercept), the hot-patch link selectors, and cargo's
/// jobserver handles, which are stale outside the original build.
pub const STRIPPED_ENV: &[&str] = &[
    WORKSPACE_WRAPPER_ENV,
    "RUSTC_WRAPPER",
    CAPTURE_ENV,
    ENV_LINK,
    ENV_ARGS_FILE,
    ENV_ERR_FILE,
    ENV_LINKER,
    "CARGO_MAKEFLAGS",
    "MAKEFLAGS",
    "MFLAGS",
];

/// Any variable with this prefix is a hot-patch control variable and is
/// stripped too.
const HOTPATCH_ENV_PREFIX: &str = "FRUST_HOTPATCH_";

/// One rustc artifact notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// The written file, resolved against the replay's working directory.
    pub path: PathBuf,
    /// What was emitted: `link`, `dep-info`, `metadata`, ...
    pub emit: String,
}

/// The result of replaying one unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayOutcome {
    pub unit: ReplayUnit,
    /// Whether rustc exited successfully. A failed compile (a user error)
    /// is an outcome, not a [`HotpatchError`].
    pub success: bool,
    /// Every artifact notification, in emission order.
    pub artifacts: Vec<Artifact>,
    /// rustc's rendered diagnostics, in emission order.
    pub diagnostics: Vec<String>,
}

impl ReplayOutcome {
    /// Every `emit: "link"` artifact.
    pub fn link_artifacts(&self) -> impl Iterator<Item = &Path> {
        self.artifacts
            .iter()
            .filter(|artifact| artifact.emit == "link")
            .map(|artifact| artifact.path.as_path())
    }

    /// The single link artifact ending in `.rlib`. None, or more than one,
    /// is [`HotpatchError::BuilderUnsupported`].
    pub fn rlib(&self) -> Result<&Path, HotpatchError> {
        let rlibs: Vec<&Path> = self
            .link_artifacts()
            .filter(|path| path.extension().is_some_and(|ext| ext == "rlib"))
            .collect();
        match rlibs.as_slice() {
            [only] => Ok(only),
            _ => Err(HotpatchError::unsupported(format!(
                "replaying {} reported {} rlib artifacts ({rlibs:?}), expected exactly one",
                self.unit,
                rlibs.len()
            ))),
        }
    }

    /// The `emit: "dep-info"` artifact, when one was written.
    pub fn dep_info(&self) -> Option<&Path> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.emit == "dep-info")
            .map(|artifact| artifact.path.as_path())
    }
}

/// The rustc arguments (program excluded) a replay of `record` runs: the
/// captured ones with every `-C linker=` form removed, plus
/// `--json=artifacts` (and `--error-format=json`, which `--json` requires)
/// when absent. A non-JSON `--error-format` is
/// [`HotpatchError::BuilderUnsupported`].
pub fn replay_args(record: &RustcRecord) -> Result<Vec<String>, HotpatchError> {
    let captured = record.rustc_args();
    let mut args = Vec::with_capacity(captured.len() + 2);
    let mut iter = captured.iter().peekable();
    while let Some(arg) = iter.next() {
        if is_linker_value(
            arg.strip_prefix("-C")
                .or_else(|| arg.strip_prefix("--codegen=")),
        ) {
            continue;
        }
        if (arg == "-C" || arg == "--codegen")
            && iter
                .peek()
                .is_some_and(|next| is_linker_value(Some(next.as_str())))
        {
            iter.next();
            continue;
        }
        args.push(arg.clone());
    }

    let error_formats = flag_values(&args, "--error-format")?;
    if let Some(other) = error_formats.iter().find(|format| *format != "json") {
        return Err(HotpatchError::unsupported(format!(
            "captured rustc invocation uses --error-format={other}; replay needs json"
        )));
    }
    let has_artifacts = flag_values(&args, "--json")?
        .iter()
        .any(|value| value.split(',').any(|item| item.trim() == "artifacts"));
    if !has_artifacts {
        if error_formats.is_empty() {
            args.push("--error-format=json".to_string());
        }
        args.push("--json=artifacts".to_string());
    }
    Ok(args)
}

/// `linker=<path>`, the codegen option being stripped (`linker-flavor=`
/// and the rest are kept).
fn is_linker_value(option: Option<&str>) -> bool {
    option.is_some_and(|option| option.starts_with("linker="))
}

/// The values of `flag` in the `flag value` and `flag=value` forms.
fn flag_values(args: &[String], flag: &str) -> Result<Vec<String>, HotpatchError> {
    let prefix = format!("{flag}=");
    let mut values = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            let value = iter
                .next()
                .ok_or_else(|| HotpatchError::unsupported(format!("`{flag}` has no value")))?;
            values.push(value.clone());
        } else if let Some(value) = arg.strip_prefix(&prefix) {
            values.push(value.to_string());
        }
    }
    Ok(values)
}

/// The captured environment minus [`STRIPPED_ENV`] and every
/// `FRUST_HOTPATCH_*` variable.
pub fn replay_env(record: &RustcRecord) -> Vec<(String, String)> {
    record
        .envs
        .iter()
        .filter(|(name, _)| !is_stripped(name))
        .cloned()
        .collect()
}

fn is_stripped(name: &str) -> bool {
    STRIPPED_ENV.contains(&name) || name.starts_with(HOTPATCH_ENV_PREFIX)
}

/// The variables a replay removes from the environment it inherits:
/// [`STRIPPED_ENV`] plus any `FRUST_HOTPATCH_*` name the record carries.
fn removed_env(record: &RustcRecord) -> Vec<String> {
    let mut names: Vec<String> = STRIPPED_ENV.iter().map(|name| name.to_string()).collect();
    for (name, _) in &record.envs {
        if is_stripped(name) && !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

/// Reads rustc's JSON stderr: artifact notifications and rendered
/// diagnostics. Non-JSON lines are skipped. A line that opens like JSON but
/// does not parse, or an artifact notification without string `artifact`
/// and `emit` fields, is [`HotpatchError::BuilderUnsupported`]. Relative
/// artifact paths resolve against `cwd`.
pub fn parse_notifications(
    stderr: &str,
    cwd: &Path,
) -> Result<(Vec<Artifact>, Vec<String>), HotpatchError> {
    let mut artifacts = Vec::new();
    let mut diagnostics = Vec::new();
    for line in stderr.lines().map(str::trim) {
        if !line.starts_with('{') {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line).map_err(|err| {
            HotpatchError::unsupported(format!("unparsable rustc notification `{line}`: {err}"))
        })?;
        let message_type = value.get("$message_type").and_then(|t| t.as_str());
        let is_artifact = match message_type {
            Some(kind) => kind == "artifact",
            None => value.get("artifact").is_some(),
        };
        if is_artifact {
            let field = |name: &str| value.get(name).and_then(|v| v.as_str());
            let (Some(path), Some(emit)) = (field("artifact"), field("emit")) else {
                return Err(HotpatchError::unsupported(format!(
                    "rustc artifact notification without string `artifact`/`emit`: `{line}`"
                )));
            };
            artifacts.push(Artifact {
                path: cwd.join(path),
                emit: emit.to_string(),
            });
        } else if let Some(rendered) = value.get("rendered").and_then(|r| r.as_str()) {
            diagnostics.push(rendered.to_string());
        }
    }
    Ok((artifacts, diagnostics))
}

/// Replays one unit's captured compile in `cwd` (see
/// [`WorkspaceGraph::replay_cwd`]). A spawn failure is
/// [`HotpatchError::Process`]; a successful compile that reports no link
/// artifact is [`HotpatchError::BuilderUnsupported`].
pub fn replay_unit(
    runner: &dyn ProcessRunner,
    unit: &ReplayUnit,
    record: &RustcRecord,
    cwd: &Path,
) -> Result<ReplayOutcome, HotpatchError> {
    let rustc = record.rustc().ok_or_else(|| {
        HotpatchError::unsupported(format!("the {unit} capture has no rustc program"))
    })?;
    let args = replay_args(record)?;
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let env = replay_env(record);
    let env: Vec<(&str, &str)> = env
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let removed = removed_env(record);
    let removed: Vec<&str> = removed.iter().map(String::as_str).collect();
    let output = runner
        .run_streaming_scrubbed(rustc, &argv, Some(cwd), &env, &removed, &mut |_| {})
        .map_err(|err| HotpatchError::Process {
            detail: format!("failed to spawn `{rustc}` to replay {unit}: {err:#}"),
        })?;
    let (artifacts, diagnostics) = parse_notifications(&output.stderr, cwd)?;
    let outcome = ReplayOutcome {
        unit: unit.clone(),
        success: output.success,
        artifacts,
        diagnostics,
    };
    if outcome.success && outcome.link_artifacts().next().is_none() {
        return Err(HotpatchError::unsupported(format!(
            "replaying {unit} succeeded but reported no link artifact"
        )));
    }
    Ok(outcome)
}

/// Replays `units` in the given order (a [`ReplayPlan`](super::graph::ReplayPlan)'s
/// `replay`). Every unit's capture is looked up before anything runs, so a
/// missing one refuses the whole replay with nothing rewritten. Stops after
/// the first failed compile, whose outcome is the last one returned. A
/// successful lib replay must report exactly one rlib.
pub fn replay_units(
    runner: &dyn ProcessRunner,
    graph: &WorkspaceGraph,
    records: &BTreeMap<RecordKey, RustcRecord>,
    units: &[ReplayUnit],
) -> Result<Vec<ReplayOutcome>, HotpatchError> {
    let planned = units
        .iter()
        .map(|unit| {
            let key = unit.record_key();
            records.get(&key).map(|record| (unit, record)).ok_or_else(|| {
                HotpatchError::unsupported(format!(
                    "no captured rustc invocation `{key}` for {unit}; a fat build must capture it"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut outcomes = Vec::with_capacity(planned.len());
    for (unit, record) in planned {
        let outcome = replay_unit(runner, unit, record, &graph.replay_cwd(unit))?;
        let success = outcome.success;
        if success && unit.kind == TargetKind::Lib {
            outcome.rlib()?;
        }
        outcomes.push(outcome);
        if !success {
            break;
        }
    }
    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{Output, StreamHandle};
    use std::sync::Mutex;

    fn unsupported<T: std::fmt::Debug>(result: Result<T, HotpatchError>) -> String {
        match result {
            Err(HotpatchError::BuilderUnsupported { detail }) => detail,
            other => panic!("expected BuilderUnsupported, got {other:?}"),
        }
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    /// One recorded replay invocation.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Call {
        program: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        env: Vec<(String, String)>,
        removed: Vec<String>,
    }

    /// Records every streaming call and answers each with the next canned
    /// output.
    struct RecordingRunner {
        calls: Mutex<Vec<Call>>,
        outputs: Mutex<Vec<Output>>,
    }

    impl RecordingRunner {
        fn new(outputs: Vec<Output>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                outputs: Mutex::new(outputs.into_iter().rev().collect()),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ProcessRunner for RecordingRunner {
        fn run(&self, cmd: &str, _args: &[&str]) -> anyhow::Result<Output> {
            anyhow::bail!("unexpected run of {cmd}")
        }

        fn run_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
            on_line: &mut dyn FnMut(&str),
        ) -> anyhow::Result<Output> {
            self.run_streaming_scrubbed(cmd, args, cwd, env, &[], on_line)
        }

        fn run_streaming_scrubbed(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
            remove_env: &[&str],
            _on_line: &mut dyn FnMut(&str),
        ) -> anyhow::Result<Output> {
            self.calls.lock().unwrap().push(Call {
                program: cmd.to_string(),
                args: args.iter().map(|a| a.to_string()).collect(),
                cwd: cwd.map(Path::to_path_buf),
                env: env
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                removed: remove_env.iter().map(|k| k.to_string()).collect(),
            });
            self.outputs
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| anyhow::anyhow!("No such file or directory (os error 2): {cmd}"))
        }

        fn spawn_streaming(
            &self,
            cmd: &str,
            _args: &[&str],
            _cwd: Option<&Path>,
            _env: &[(&str, &str)],
        ) -> anyhow::Result<StreamHandle> {
            anyhow::bail!("unexpected spawn of {cmd}")
        }
    }

    const OUT: &str = "/p/my-app/target/debug/deps";

    /// The template lib's capture as cargo records it: three crate types,
    /// no `-C extra-filename`, a `-C linker=` the fat build routed to
    /// `frust`, and `--json` without `artifacts`.
    fn template_lib_record() -> RustcRecord {
        RustcRecord {
            args: strings(&[
                "/rust/bin/rustc",
                "--crate-name",
                "my_app",
                "--edition=2024",
                "src/lib.rs",
                "--error-format=json",
                "--json=diagnostic-rendered-ansi,future-incompat",
                "--crate-type",
                "cdylib",
                "--crate-type",
                "staticlib",
                "--crate-type",
                "rlib",
                "--emit=dep-info,link",
                "-C",
                "linker=/bin/frust",
                "-C",
                "metadata=70ca96c8873de460",
                "--out-dir",
                OUT,
                "-Clinker=/bin/frust",
                "-C",
                "linker-flavor=gcc",
            ]),
            envs: vec![
                (
                    "CARGO_MAKEFLAGS".to_string(),
                    "-j --jobserver-fds=3,4".to_string(),
                ),
                ("CARGO_PKG_NAME".to_string(), "my-app".to_string()),
                (CAPTURE_ENV.to_string(), "/t/scope".to_string()),
                ("FRUST_HOTPATCH_EXTRA".to_string(), "1".to_string()),
                (ENV_LINK.to_string(), "no-link".to_string()),
                (WORKSPACE_WRAPPER_ENV.to_string(), "/bin/frust".to_string()),
            ],
            crate_types: strings(&["cdylib", "staticlib", "rlib"]),
        }
    }

    fn template_graph() -> WorkspaceGraph {
        const ID: &str = "path+file:///p/my-app#0.1.0";
        let metadata = serde_json::json!({
            "packages": [{
                "id": ID, "name": "my-app", "source": null,
                "manifest_path": "/p/my-app/Cargo.toml",
                "targets": [
                    {"name": "my_app", "kind": ["cdylib", "staticlib", "rlib"],
                     "src_path": "/p/my-app/src/lib.rs"},
                    {"name": "my-app", "kind": ["bin"], "src_path": "/p/my-app/src/main.rs"},
                ],
            }],
            "workspace_members": [ID],
            "resolve": {"nodes": [{"id": ID, "deps": []}], "root": ID},
            "workspace_root": "/p/my-app",
        });
        WorkspaceGraph::from_metadata(&metadata.to_string(), "my-app", None).unwrap()
    }

    fn lib_unit() -> ReplayUnit {
        ReplayUnit::lib("my-app", "my_app")
    }

    /// rustc's stderr for the template lib: dep-info, a warning, then one
    /// link artifact per crate type.
    fn lib_stderr() -> String {
        [
            format!(r#"{{"$message_type":"artifact","artifact":"{OUT}/my_app.d","emit":"dep-info"}}"#),
            r#"{"$message_type":"diagnostic","message":"unused","level":"warning","rendered":"warning: unused\n"}"#.to_string(),
            format!(r#"{{"$message_type":"artifact","artifact":"{OUT}/libmy_app.dylib","emit":"link"}}"#),
            format!(r#"{{"$message_type":"artifact","artifact":"{OUT}/libmy_app.a","emit":"link"}}"#),
            format!(r#"{{"$message_type":"artifact","artifact":"{OUT}/libmy_app.rlib","emit":"link"}}"#),
        ]
        .join("\n")
    }

    fn ok(stderr: String) -> Output {
        Output {
            success: true,
            stdout: String::new(),
            stderr,
        }
    }

    fn records(pairs: &[(&ReplayUnit, RustcRecord)]) -> BTreeMap<RecordKey, RustcRecord> {
        pairs
            .iter()
            .map(|(unit, record)| (unit.record_key(), record.clone()))
            .collect()
    }

    #[test]
    fn the_template_lib_replays_all_crate_types_without_the_linker_and_finds_its_rlib() {
        let graph = template_graph();
        let record = template_lib_record();
        assert!(
            !record.args.iter().any(|arg| arg.contains("extra-filename")),
            "the fixture has no -C extra-filename"
        );
        let runner = RecordingRunner::new(vec![ok(lib_stderr())]);
        let outcomes = replay_units(
            &runner,
            &graph,
            &records(&[(&lib_unit(), record)]),
            &[lib_unit()],
        )
        .unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.program, "/rust/bin/rustc");
        assert_eq!(
            call.args,
            strings(&[
                "--crate-name",
                "my_app",
                "--edition=2024",
                "src/lib.rs",
                "--error-format=json",
                "--json=diagnostic-rendered-ansi,future-incompat",
                "--crate-type",
                "cdylib",
                "--crate-type",
                "staticlib",
                "--crate-type",
                "rlib",
                "--emit=dep-info,link",
                "-C",
                "metadata=70ca96c8873de460",
                "--out-dir",
                OUT,
                "-C",
                "linker-flavor=gcc",
                "--json=artifacts",
            ])
        );
        assert_eq!(call.cwd.as_deref(), Some(Path::new("/p/my-app")));
        assert_eq!(
            call.env,
            vec![("CARGO_PKG_NAME".to_string(), "my-app".to_string())]
        );
        for name in [
            WORKSPACE_WRAPPER_ENV,
            CAPTURE_ENV,
            ENV_LINK,
            "CARGO_MAKEFLAGS",
            "FRUST_HOTPATCH_EXTRA",
        ] {
            assert!(call.removed.iter().any(|r| r == name), "{name} is removed");
        }

        let outcome = &outcomes[0];
        assert!(outcome.success);
        assert_eq!(
            outcome.rlib().unwrap(),
            Path::new("/p/my-app/target/debug/deps/libmy_app.rlib")
        );
        assert_eq!(outcome.link_artifacts().count(), 3);
        assert_eq!(
            outcome.dep_info(),
            Some(Path::new("/p/my-app/target/debug/deps/my_app.d"))
        );
        assert_eq!(outcome.diagnostics, vec!["warning: unused\n".to_string()]);
    }

    #[test]
    fn replay_args_keep_existing_artifact_json_and_add_error_format_when_missing() {
        let mut record = template_lib_record();
        record.args = strings(&[
            "rustc",
            "--crate-name",
            "x",
            "--error-format",
            "json",
            "--json",
            "diagnostic-rendered-ansi,artifacts,future-incompat",
            "--codegen",
            "linker=/bin/frust",
            "--codegen=linker=/bin/frust",
        ]);
        assert_eq!(
            replay_args(&record).unwrap(),
            strings(&[
                "--crate-name",
                "x",
                "--error-format",
                "json",
                "--json",
                "diagnostic-rendered-ansi,artifacts,future-incompat",
            ])
        );

        record.args = strings(&["rustc", "--crate-name", "x"]);
        assert_eq!(
            replay_args(&record).unwrap(),
            strings(&[
                "--crate-name",
                "x",
                "--error-format=json",
                "--json=artifacts"
            ])
        );

        record.args = strings(&["rustc", "--crate-name", "x", "--error-format=human"]);
        assert!(unsupported(replay_args(&record)).contains("human"));
    }

    #[test]
    fn a_missing_capture_refuses_before_anything_runs() {
        let graph = template_graph();
        let runner = RecordingRunner::new(vec![ok(lib_stderr())]);
        let bin = ReplayUnit::bin("my-app", "my-app");
        let detail = unsupported(replay_units(
            &runner,
            &graph,
            &records(&[(&lib_unit(), template_lib_record())]),
            &[lib_unit(), bin],
        ));
        assert!(detail.contains("my_app.bin"), "{detail}");
        assert!(
            runner.calls().is_empty(),
            "nothing is rewritten on a refusal"
        );
    }

    #[test]
    fn unparsable_notifications_are_builder_unsupported() {
        let cwd = Path::new("/p");
        assert!(
            unsupported(parse_notifications(r#"{"artifact":"/t/libx.rlib","#, cwd))
                .contains("unparsable")
        );
        unsupported(parse_notifications(
            r#"{"$message_type":"artifact","artifact":3,"emit":"link"}"#,
            cwd,
        ));
        unsupported(parse_notifications(r#"{"artifact":"/t/libx.rlib"}"#, cwd));

        let graph = template_graph();
        let runner = RecordingRunner::new(vec![ok(r#"{"artifact": null, "emit": "link"}"#.into())]);
        unsupported(replay_units(
            &runner,
            &graph,
            &records(&[(&lib_unit(), template_lib_record())]),
            &[lib_unit()],
        ));
    }

    #[test]
    fn old_style_notifications_and_relative_paths_parse() {
        let (artifacts, diagnostics) = parse_notifications(
            "plain text line\n{\"artifact\":\"target/libx.rlib\",\"emit\":\"link\"}\n",
            Path::new("/w"),
        )
        .unwrap();
        assert_eq!(
            artifacts,
            vec![Artifact {
                path: PathBuf::from("/w/target/libx.rlib"),
                emit: "link".to_string(),
            }]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn a_lib_replay_without_exactly_one_rlib_is_refused() {
        let graph = template_graph();
        let no_rlib = format!(
            r#"{{"$message_type":"artifact","artifact":"{OUT}/libmy_app.dylib","emit":"link"}}"#
        );
        let runner = RecordingRunner::new(vec![ok(no_rlib)]);
        let detail = unsupported(replay_units(
            &runner,
            &graph,
            &records(&[(&lib_unit(), template_lib_record())]),
            &[lib_unit()],
        ));
        assert!(detail.contains("rlib"), "{detail}");

        let runner = RecordingRunner::new(vec![ok(String::new())]);
        assert!(
            unsupported(replay_units(
                &runner,
                &graph,
                &records(&[(&lib_unit(), template_lib_record())]),
                &[lib_unit()],
            ))
            .contains("no link artifact")
        );
    }

    #[test]
    fn a_failed_compile_stops_the_replay_and_carries_its_diagnostics() {
        let graph = template_graph();
        let bin = ReplayUnit::bin("my-app", "my-app");
        let mut bin_record = template_lib_record();
        bin_record.crate_types = strings(&["bin"]);
        let failed = Output {
            success: false,
            stdout: String::new(),
            stderr: r#"{"$message_type":"diagnostic","rendered":"error[E0425]: x\n"}"#.to_string(),
        };
        let runner = RecordingRunner::new(vec![failed, ok(lib_stderr())]);
        let outcomes = replay_units(
            &runner,
            &graph,
            &records(&[(&lib_unit(), template_lib_record()), (&bin, bin_record)]),
            &[lib_unit(), bin],
        )
        .unwrap();
        assert_eq!(outcomes.len(), 1, "the bin never runs after the lib failed");
        assert!(!outcomes[0].success);
        assert_eq!(
            outcomes[0].diagnostics,
            vec!["error[E0425]: x\n".to_string()]
        );
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn a_spawn_failure_is_a_process_error() {
        let graph = template_graph();
        let runner = RecordingRunner::new(Vec::new());
        assert!(matches!(
            replay_units(
                &runner,
                &graph,
                &records(&[(&lib_unit(), template_lib_record())]),
                &[lib_unit()],
            ),
            Err(HotpatchError::Process { .. })
        ));
    }

    /// The real toolchain: a one-package lib captured with three crate
    /// types, no `-C extra-filename`, and a `-C linker=` pointing at a
    /// program that does not exist (the cdylib link would fail if replay
    /// kept it). The rlib comes back from rustc's own notifications.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_real_rustc_replay_of_a_three_crate_type_lib_reports_its_rlib() {
        use crate::process::RealProcessRunner;
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "frust-drive-replay-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub const GREETING: &str = include_str!(\"greeting.txt\");\n\
             pub fn greet() -> &'static str { GREETING }\n",
        )
        .unwrap();
        std::fs::write(root.join("src/greeting.txt"), "hello").unwrap();
        let out = root.join("out");
        let out_text = out.to_str().unwrap();
        let record = RustcRecord {
            args: strings(&[
                "rustc",
                "--crate-name",
                "fixture",
                "--edition=2021",
                "src/lib.rs",
                "--error-format=json",
                "--json=diagnostic-rendered-ansi",
                "--crate-type",
                "cdylib",
                "--crate-type",
                "staticlib",
                "--crate-type",
                "rlib",
                "--emit=dep-info,link",
                "-C",
                "debuginfo=0",
                "-Clinker=/nonexistent/frust-hotpatch-linker",
                "--out-dir",
                out_text,
            ]),
            envs: Vec::new(),
            crate_types: strings(&["cdylib", "staticlib", "rlib"]),
        };
        let unit = ReplayUnit::lib("fixture", "fixture");
        let outcome = replay_unit(&RealProcessRunner, &unit, &record, &root).unwrap();
        assert!(outcome.success, "{:?}", outcome.diagnostics);
        assert_eq!(outcome.rlib().unwrap(), out.join("libfixture.rlib"));
        assert!(outcome.rlib().unwrap().is_file());
        assert_eq!(outcome.link_artifacts().count(), 3);
        let dep_info = outcome.dep_info().expect("dep-info is reported");
        let listed = super::super::graph::read_dep_info(dep_info, &root).unwrap();
        assert!(listed.contains(&root.join("src/greeting.txt")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
