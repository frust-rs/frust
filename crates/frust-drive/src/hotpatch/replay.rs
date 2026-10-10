//! Workspace replay: recompile modified (package, target) units by running
//! their captured rustc invocations again.
//!
//! Each captured invocation runs again with `-Clinker` stripped (a replayed
//! crate must produce real outputs, not re-enter the no-link interception)
//! and `--json=artifacts` forced when absent ([`replay_args`], which the
//! iOS tip capture uses as is; the session's image unit replay narrows it
//! with [`replay_args_image`]). A lib replayed here
//! ([`replay_unit`]) emits only its rlib ([`replay_args_rlib_only`]): the
//! hot run reads nothing else from it, and the cdylib link (MSVC
//! `link.exe`, ~3 s) and staticlib archive it was captured with are most of
//! a Windows change's compile time. Replay writes at the same paths cargo
//! originally wrote to, so anything that must read the fat build's outputs
//! (the base DWARF) reads them before the first replay.
//!
//! Outputs are read from rustc's artifact notifications
//! (`{"artifact": <path>, "emit": "link"}` on stderr), the parse
//! dioxus-cli 0.7.10 applies to its tip build. The rlib to link is the link
//! artifact ending in `.rlib`, whatever its name: dioxus-cli's
//! `-C extra-filename` path and `lib<crate>-*.rlib` glob both miss the
//! `lib<crate>.rlib` cargo writes for a `["cdylib", "staticlib", "rlib"]`
//! lib, and the rlib-only replay keeps that name. A missing capture or an
//! unparsable notification is [`HotpatchError::BuilderUnsupported`]. See
//! `docs/CLI_ARCHITECTURE.md`.

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

/// [`replay_args`] narrowed to the rlib a lib replay is read for: a `Lib`
/// record whose `--crate-type` values hold `rlib` or `lib` beside other
/// types keeps one `--crate-type rlib` (space or `=` form, at the first
/// such occurrence) and drops every other `--crate-type` flag, comma lists
/// included. `--emit`, `-C extra-filename`, `--out-dir` and the rest are
/// untouched, so the rlib keeps its name. A `Bin` record, a single-type
/// lib, and a lib with no rlib/lib type (it has no rlib to narrow to, and
/// [`replay_units`] refuses its outcome) are returned as [`replay_args`]
/// builds them.
pub fn replay_args_rlib_only(record: &RustcRecord) -> Result<Vec<String>, HotpatchError> {
    let args = replay_args(record)?;
    if TargetKind::of(&record.crate_types) == TargetKind::Bin {
        return Ok(args);
    }
    let mut types: Vec<String> = Vec::new();
    for value in flag_values(&args, CRATE_TYPE)? {
        for ty in value.split(',').map(str::trim) {
            if !types.iter().any(|known| known == ty) {
                types.push(ty.to_string());
            }
        }
    }
    if types.len() < 2 || !types.iter().any(|ty| is_rlib_type(ty)) {
        return Ok(args);
    }

    let equals_prefix = format!("{CRATE_TYPE}=");
    let mut narrowed = Vec::with_capacity(args.len());
    let mut kept = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let (value, equals_form) = if arg == CRATE_TYPE {
            // `flag_values` above refused a trailing `--crate-type`.
            let Some(value) = iter.next() else { break };
            (value, false)
        } else if let Some(value) = arg.strip_prefix(&equals_prefix) {
            (value.to_string(), true)
        } else {
            narrowed.push(arg);
            continue;
        };
        if kept || !value.split(',').map(str::trim).any(is_rlib_type) {
            continue;
        }
        kept = true;
        if equals_form {
            narrowed.push(format!("{CRATE_TYPE}=rlib"));
        } else {
            narrowed.push(CRATE_TYPE.to_string());
            narrowed.push("rlib".to_string());
        }
    }
    Ok(narrowed)
}

/// [`replay_args`] narrowed to what a lib image's patch needs: the
/// `staticlib`, `dylib` and `proc-macro` crate types are dropped from every
/// `--crate-type` flag (space or `=` form, comma lists included) and the rest
/// (`cdylib`, whose intercepted link names the fresh objects, and `rlib`)
/// stay in place. A scaffolded app's lib declares
/// `["cdylib", "staticlib", "rlib"]`, and the staticlib bundles every
/// dependency (~1.1 GB for a large app) that a patch never reads.
///
/// The `rlib` is kept: it costs one archive of the crate's own objects,
/// nothing links it (`modified_rlibs` excludes the image unit) and the
/// codegen is shared with the cdylib, so keeping it leaves the objects the
/// patch links byte-for-byte what [`replay_args`] produces. A record whose
/// types hold none of the dropped ones, a `Bin`, and a record whose every
/// type would be dropped (a staticlib-only lib, which has nothing else to
/// emit) are returned as [`replay_args`] builds them. The iOS simulator's
/// tip capture keeps [`replay_args`]: Xcode links the staticlib it writes.
pub fn replay_args_image(record: &RustcRecord) -> Result<Vec<String>, HotpatchError> {
    let args = replay_args(record)?;
    if TargetKind::of(&record.crate_types) == TargetKind::Bin {
        return Ok(args);
    }
    let mut dropped = false;
    let mut kept = false;
    for value in flag_values(&args, CRATE_TYPE)? {
        for ty in value.split(',').map(str::trim) {
            if is_image_unused_type(ty) {
                dropped = true;
            } else {
                kept = true;
            }
        }
    }
    if !dropped || !kept {
        return Ok(args);
    }

    let equals_prefix = format!("{CRATE_TYPE}=");
    let mut narrowed = Vec::with_capacity(args.len());
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let (value, equals_form) = if arg == CRATE_TYPE {
            // `flag_values` above refused a trailing `--crate-type`.
            let Some(value) = iter.next() else { break };
            (value, false)
        } else if let Some(value) = arg.strip_prefix(&equals_prefix) {
            (value.to_string(), true)
        } else {
            narrowed.push(arg);
            continue;
        };
        let remaining: Vec<&str> = value
            .split(',')
            .map(str::trim)
            .filter(|ty| !is_image_unused_type(ty))
            .collect();
        if remaining.is_empty() {
            continue;
        }
        let joined = if remaining.len() == value.split(',').count() {
            value.clone()
        } else {
            remaining.join(",")
        };
        if equals_form {
            narrowed.push(format!("{CRATE_TYPE}={joined}"));
        } else {
            narrowed.push(CRATE_TYPE.to_string());
            narrowed.push(joined);
        }
    }
    Ok(narrowed)
}

/// The crate types a lib image's patch never reads.
fn is_image_unused_type(ty: &str) -> bool {
    matches!(ty, "staticlib" | "dylib" | "proc-macro")
}

const CRATE_TYPE: &str = "--crate-type";

/// `rlib`, or `lib` (rustc's default library type, an rlib).
fn is_rlib_type(ty: &str) -> bool {
    ty == "rlib" || ty == "lib"
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
/// [`WorkspaceGraph::replay_cwd`]) with [`replay_args_rlib_only`], so a lib
/// emits only its rlib. Not for the image unit, whose intercepted link
/// needs every captured crate type. A spawn failure is
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
    let args = replay_args_rlib_only(record)?;
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
/// successful lib replay must report exactly one rlib, the only output it
/// emits ([`replay_unit`]).
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

    /// rustc's stderr for the template lib's rlib-only replay: dep-info, a
    /// warning, then the rlib's link artifact.
    fn lib_stderr() -> String {
        [
            format!(r#"{{"$message_type":"artifact","artifact":"{OUT}/my_app.d","emit":"dep-info"}}"#),
            r#"{"$message_type":"diagnostic","message":"unused","level":"warning","rendered":"warning: unused\n"}"#.to_string(),
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
    fn the_template_lib_replays_only_its_rlib_without_the_linker_and_finds_it() {
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
        assert_eq!(outcome.link_artifacts().count(), 1);
        assert_eq!(
            outcome.dep_info(),
            Some(Path::new("/p/my-app/target/debug/deps/my_app.d"))
        );
        assert_eq!(outcome.diagnostics, vec!["warning: unused\n".to_string()]);
    }

    /// The image unit's replay (`replay_args`, which the session's
    /// intercepted tip replay runs) keeps every captured crate type; the
    /// same record through the libs path (`replay_units`) carries one
    /// `--crate-type rlib` and nothing else differs.
    #[test]
    fn the_image_path_keeps_every_crate_type_and_the_libs_path_only_the_rlib() {
        let record = template_lib_record();
        let image = replay_args(&record).unwrap();
        assert_eq!(
            image,
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

        let runner = RecordingRunner::new(vec![ok(lib_stderr())]);
        replay_units(
            &runner,
            &template_graph(),
            &records(&[(&lib_unit(), record)]),
            &[lib_unit()],
        )
        .unwrap();
        let libs = runner.calls().remove(0).args;
        let crate_types: Vec<&str> = libs
            .windows(2)
            .filter(|pair| pair[0] == "--crate-type")
            .map(|pair| pair[1].as_str())
            .collect();
        assert_eq!(crate_types, vec!["rlib"]);
        assert!(!libs.iter().any(|arg| arg.starts_with("--crate-type=")));
        let mut expected = image.clone();
        expected.drain(6..10);
        assert_eq!(
            libs, expected,
            "only the cdylib/staticlib flags are dropped"
        );
    }

    fn lib_record(args: &[&str]) -> RustcRecord {
        let mut argv = vec!["rustc", "--crate-name", "x"];
        argv.extend_from_slice(args);
        argv.push("--json=artifacts");
        RustcRecord {
            args: strings(&argv),
            envs: Vec::new(),
            crate_types: crate::hotpatch::capture::crate_types(&strings(&argv)).unwrap(),
        }
    }

    /// The narrowed argv, minus the `--crate-name x` prefix and the
    /// trailing `--json=artifacts`.
    fn narrowed(args: &[&str]) -> Vec<String> {
        let all = replay_args_rlib_only(&lib_record(args)).unwrap();
        all[2..all.len() - 1].to_vec()
    }

    #[test]
    fn comma_lists_and_the_equals_form_narrow_to_one_rlib() {
        let emit = "--emit=dep-info,link";
        assert_eq!(
            narrowed(&["--crate-type=cdylib,rlib", emit]),
            strings(&["--crate-type=rlib", emit])
        );
        assert_eq!(
            narrowed(&["--crate-type", "staticlib,rlib,cdylib", emit]),
            strings(&["--crate-type", "rlib", emit])
        );
        assert_eq!(
            narrowed(&[
                "--crate-type",
                "cdylib,staticlib",
                emit,
                "--crate-type=lib",
                "--crate-type",
                "dylib",
            ]),
            strings(&[emit, "--crate-type=rlib"]),
            "the first rlib/lib occurrence keeps its position and form"
        );
        assert_eq!(
            narrowed(&[
                "--crate-type=staticlib",
                "--crate-type",
                "rlib",
                "--crate-type=cdylib",
                "--crate-type=rlib",
            ]),
            strings(&["--crate-type", "rlib"])
        );
        assert_eq!(
            narrowed(&[
                "--crate-type",
                "dylib",
                "-C",
                "extra-filename=-abc",
                "--crate-type",
                "lib"
            ]),
            strings(&["-C", "extra-filename=-abc", "--crate-type", "rlib"])
        );
    }

    #[test]
    fn a_bin_a_single_type_lib_and_a_lib_without_an_rlib_replay_as_captured() {
        for args in [
            &["--crate-type", "bin", "--emit=dep-info,link"][..],
            &[
                "--crate-type",
                "bin",
                "--crate-type=rlib",
                "--crate-type=cdylib",
            ][..],
            &["--crate-type", "cdylib", "--crate-type=staticlib"][..],
            &["--crate-type=cdylib,staticlib"][..],
            &["--crate-type", "lib"][..],
            &["--crate-type=rlib"][..],
        ] {
            let record = lib_record(args);
            assert_eq!(
                replay_args_rlib_only(&record).unwrap(),
                replay_args(&record).unwrap(),
                "{args:?}"
            );
        }

        let bin = ReplayUnit::bin("my-app", "my-app");
        let mut bin_record = template_lib_record();
        bin_record.crate_types = strings(&["bin"]);
        let expected = replay_args(&bin_record).unwrap();
        let bin_stderr =
            format!(r#"{{"$message_type":"artifact","artifact":"{OUT}/my_app","emit":"link"}}"#);
        let runner = RecordingRunner::new(vec![ok(bin_stderr)]);
        replay_units(
            &runner,
            &template_graph(),
            &records(&[(&bin, bin_record)]),
            &[bin],
        )
        .unwrap();
        assert_eq!(
            runner.calls()[0].args,
            expected,
            "a bin replays byte-identically"
        );
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

    fn image(args: &[&str]) -> Vec<String> {
        replay_args_image(&lib_record(args)).unwrap()
    }

    #[test]
    fn the_image_replay_drops_the_staticlib_and_keeps_the_cdylib_and_rlib() {
        let record = template_lib_record();
        let full = replay_args(&record).unwrap();
        let image = replay_args_image(&record).unwrap();
        let mut expected = full.clone();
        expected.drain(8..10);
        assert_eq!(image, expected, "only the staticlib pair is dropped");
        let crate_types: Vec<&str> = image
            .windows(2)
            .filter(|pair| pair[0] == "--crate-type")
            .map(|pair| pair[1].as_str())
            .collect();
        assert_eq!(crate_types, vec!["cdylib", "rlib"]);
    }

    #[test]
    fn the_image_replay_narrows_the_equals_form_and_comma_lists_in_place() {
        assert_eq!(
            image(&[
                "--crate-type=cdylib",
                "--crate-type=staticlib",
                "--crate-type=rlib"
            ]),
            strings(&[
                "--crate-name",
                "x",
                "--crate-type=cdylib",
                "--crate-type=rlib",
                "--json=artifacts"
            ])
        );
        assert_eq!(
            image(&["--crate-type", "cdylib,staticlib,rlib", "--emit=link"]),
            strings(&[
                "--crate-name",
                "x",
                "--crate-type",
                "cdylib,rlib",
                "--emit=link",
                "--json=artifacts"
            ])
        );
        assert_eq!(
            image(&["--crate-type=staticlib,dylib,cdylib", "--emit=link"]),
            strings(&[
                "--crate-name",
                "x",
                "--crate-type=cdylib",
                "--emit=link",
                "--json=artifacts"
            ])
        );
        assert_eq!(
            image(&["--crate-type", "proc-macro", "--crate-type", "cdylib"]),
            strings(&[
                "--crate-name",
                "x",
                "--crate-type",
                "cdylib",
                "--json=artifacts"
            ])
        );
    }

    #[test]
    fn the_image_replay_of_a_record_without_a_staticlib_is_byte_identical() {
        for args in [
            &["--crate-type", "cdylib", "--crate-type", "rlib"][..],
            &["--crate-type=cdylib,rlib"][..],
            &["--crate-type", "cdylib"][..],
            &["--crate-type", "staticlib"][..],
            &["--crate-type=staticlib,dylib"][..],
        ] {
            let record = lib_record(args);
            assert_eq!(
                replay_args_image(&record).unwrap(),
                replay_args(&record).unwrap(),
                "{args:?}"
            );
        }
        let bin_record = RustcRecord {
            args: strings(&["rustc", "--crate-type", "bin", "--crate-type", "staticlib"]),
            envs: Vec::new(),
            crate_types: strings(&["bin"]),
        };
        assert_eq!(
            replay_args_image(&bin_record).unwrap(),
            replay_args(&bin_record).unwrap()
        );
    }

    /// The real toolchain: the image replay of a three-type lib leaves the
    /// cdylib and the rlib in the out-dir and no archive.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_real_rustc_image_replay_of_a_three_crate_type_lib_writes_no_staticlib() {
        use crate::process::{ProcessRunner, RealProcessRunner};
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "frust-drive-replay-image-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn greet() -> u8 { 7 }\n").unwrap();
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
                "--crate-type",
                "cdylib",
                "--crate-type",
                "staticlib",
                "--crate-type",
                "rlib",
                "--emit=dep-info,link",
                "-C",
                "debuginfo=0",
                "--out-dir",
                out_text,
            ]),
            envs: Vec::new(),
            crate_types: strings(&["cdylib", "staticlib", "rlib"]),
        };
        let args = replay_args_image(&record).unwrap();
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = RealProcessRunner
            .run_streaming_scrubbed("rustc", &argv, Some(&root), &[], &[], &mut |_| {})
            .unwrap();
        assert!(output.success, "{}", output.stderr);
        let mut written: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        written.sort();
        assert!(
            written.iter().any(|name| name == "libfixture.rlib"),
            "{written:?}"
        );
        assert!(
            written
                .iter()
                .any(|name| name.ends_with(".so") || name.ends_with(".dylib")),
            "{written:?}"
        );
        assert!(
            !written.iter().any(|name| name.ends_with(".a")),
            "{written:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The real toolchain: a one-package lib captured with three crate
    /// types, no `-C extra-filename`, and a `-C linker=` pointing at a
    /// program that does not exist (the cdylib link would fail if replay
    /// kept it). The replay emits only the rlib, which comes back from
    /// rustc's own notifications; the out-dir holds no cdylib or staticlib.
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
        assert_eq!(outcome.link_artifacts().count(), 1);
        let mut written: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        written.sort();
        assert_eq!(
            written,
            vec!["fixture.d".to_string(), "libfixture.rlib".to_string()],
            "no cdylib or staticlib output"
        );
        let dep_info = outcome.dep_info().expect("dep-info is reported");
        let listed = super::super::graph::read_dep_info(dep_info, &root).unwrap();
        assert!(listed.contains(&root.join("src/greeting.txt")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The real toolchain over a framework-style path dependency: `shared`
    /// lives outside the workspace root (so it compiled in its own
    /// directory, its source path relative to it, at `opt-level = 1` as
    /// `[profile.dev.package."*"]` builds it) and the member `app` holds its
    /// `Offset` by value. Once the fat build's `shared` capture makes it
    /// replayable, an edit to it replays `shared` then `app` against the
    /// fresh rlib; a body edit passes the layout gate against the base, a
    /// field added to `Offset` is refused naming both types.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_real_rustc_replay_of_an_edited_path_dependency_cascades_to_the_tip_under_the_gate() {
        use super::super::capture::RecordKey;
        use super::super::graph::{ModifiedSet, PathClass};
        use super::super::layout::{self, LayoutChanged};
        use crate::process::RealProcessRunner;
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir().join(format!(
            "frust-drive-replay-path-dep-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("ws");
        let shared = base.join("frust").join("shared");
        let out = base.join("target").join("debug").join("deps");
        for dir in [root.join("app/src"), shared.join("src"), out.clone()] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let shared_source = shared.join("src/lib.rs");
        let write_shared = |value: &str, field: &str| {
            std::fs::write(
                &shared_source,
                format!(
                    "pub struct Offset {{ pub value: u64,{field} }}\n\
                     #[inline(never)]\n\
                     pub fn offset() -> Offset {{ Offset {{ value: {value},{} }} }}\n",
                    if field.is_empty() { "" } else { " extra: 0" }
                ),
            )
            .unwrap();
        };
        write_shared("0", "");
        std::fs::write(
            root.join("app/src/lib.rs"),
            "pub struct Reading { pub value: u64, pub offset: shared::Offset }\n\
             pub fn reading() -> Reading {\n\
                 let offset = shared::offset();\n\
                 Reading { value: 1 + offset.value, offset }\n\
             }\n",
        )
        .unwrap();

        let (_, rustc) = super::super::layout::fixture::toolchain();
        let rustc = rustc.map_or_else(|| "rustc".to_string(), |p| p.display().to_string());
        let out_text = out.to_str().unwrap();
        let compile = |name: &str, src: &str, extra: &[String]| RustcRecord {
            args: [
                rustc.as_str(),
                "--crate-name",
                name,
                "--edition=2024",
                src,
                "--error-format=json",
                "--crate-type",
                "lib",
                "--emit=dep-info,metadata,link",
                "-C",
                "debuginfo=2",
                "--out-dir",
                out_text,
            ]
            .iter()
            .map(|arg| arg.to_string())
            .chain(extra.iter().cloned())
            .collect(),
            envs: Vec::new(),
            crate_types: strings(&["lib"]),
        };
        let shared_rlib = out.join("libshared-aa.rlib");
        let records: BTreeMap<RecordKey, RustcRecord> = [
            (
                RecordKey::parse("shared.lib").unwrap(),
                compile(
                    "shared",
                    "src/lib.rs",
                    &strings(&["-Copt-level=1", "-Cextra-filename=-aa"]),
                ),
            ),
            (
                RecordKey::parse("app.lib").unwrap(),
                compile(
                    "app",
                    "app/src/lib.rs",
                    &[
                        "-Cextra-filename=-bb".to_string(),
                        "--extern".to_string(),
                        format!("shared={}", shared_rlib.display()),
                    ],
                ),
            ),
            (
                RecordKey::parse("app.bin").unwrap(),
                RustcRecord {
                    args: strings(&["rustc"]),
                    envs: Vec::new(),
                    crate_types: strings(&["bin"]),
                },
            ),
        ]
        .into_iter()
        .collect();

        const APP: &str = "path+file:///ws/app#0.1.0";
        const SHARED: &str = "path+file:///frust/shared#0.1.0";
        let metadata = serde_json::json!({
            "packages": [
                {"id": APP, "name": "app", "source": null,
                 "manifest_path": root.join("app/Cargo.toml"),
                 "targets": [
                    {"name": "app", "kind": ["lib"], "src_path": root.join("app/src/lib.rs")},
                    {"name": "app", "kind": ["bin"], "src_path": root.join("app/src/main.rs")},
                 ]},
                {"id": SHARED, "name": "shared", "source": null,
                 "manifest_path": shared.join("Cargo.toml"),
                 "targets": [{"name": "shared", "kind": ["lib"], "src_path": &shared_source}]},
            ],
            "workspace_members": [APP],
            "resolve": {"nodes": [
                {"id": APP, "deps": [{"name": "shared", "pkg": SHARED,
                                      "dep_kinds": [{"kind": null, "target": null}]}]},
                {"id": SHARED, "deps": []},
            ], "root": APP},
            "workspace_root": &root,
        });
        let mut graph = WorkspaceGraph::from_metadata(&metadata.to_string(), "app", None).unwrap();
        assert_eq!(
            graph.replay_non_members(&records),
            vec!["shared".to_string()]
        );
        let crates = vec!["app".to_string(), "shared".to_string()];
        let replay = |units: &[ReplayUnit]| -> Vec<PathBuf> {
            replay_units(&RealProcessRunner, &graph, &records, units)
                .unwrap()
                .iter()
                .map(|outcome| {
                    assert!(
                        outcome.success,
                        "{}: {:?}",
                        outcome.unit, outcome.diagnostics
                    );
                    outcome.rlib().unwrap().to_path_buf()
                })
                .collect()
        };

        // The fat build's compiles, in dependency order: the base table.
        let shared_unit = ReplayUnit::lib("shared", "shared");
        let app_unit = ReplayUnit::lib("app", "app");
        let fat = replay(&[shared_unit.clone(), app_unit.clone()]);
        assert_eq!(fat[0], shared_rlib);
        let accepted = layout::extract(&fat, &crates).unwrap().table;
        assert!(accepted.get("shared::Offset").is_some());

        let edit = |value: &str, field: &str| {
            write_shared(value, field);
            let units = match graph.classify(&shared_source) {
                PathClass::Replayable { units } => units,
                other => panic!("expected replayable, got {other:?}"),
            };
            let plan = ModifiedSet::new().record_change(&graph, &units).unwrap();
            assert_eq!(plan.replay, vec![shared_unit.clone(), app_unit.clone()]);
            let candidate = layout::extract(&replay(&plan.replay), &crates)
                .unwrap()
                .table;
            layout::diff(&candidate, &accepted)
        };
        assert_eq!(
            edit("1", ""),
            Vec::<LayoutChanged>::new(),
            "a body edit passes"
        );
        let changed: Vec<String> = edit("1", " pub extra: u64,")
            .into_iter()
            .map(|change| change.type_path)
            .collect();
        for ty in ["shared::Offset", "app::Reading"] {
            assert!(changed.iter().any(|path| path == ty), "{ty} in {changed:?}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
