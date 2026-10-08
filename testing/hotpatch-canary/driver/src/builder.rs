//! The hot-patch session minus its transport: `frust_drive::hotpatch`'s
//! builder pieces driven directly, in the order `hotpatch::session` runs them
//! (`start_desktop`, then `on_change`'s compile, gate and link), with the
//! devtools upload replaced by the caller handing the table to the app.
//!
//! The session keeps a few small helpers private (`tip_objects`,
//! `typed_objects`, `member_rlibs`, `replayable_crates`, `seed_dep_info`, the
//! `cargo metadata` / `rustc -vV` / environment readers). They are mirrored
//! here line for line, so a change to them in `session.rs` must be mirrored
//! too; everything else is called through the public API.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use frust_drive::doctor::{EnvLookup, RealEnv};
use frust_drive::hotpatch::capture::{
    self, RecordKey, RustcRecord, ScopeInputs, TargetKind, load_records, prepare_scope_dir,
};
use frust_drive::hotpatch::fat_link::{self, FatLinkRequest, LinkerFlavor};
use frust_drive::hotpatch::graph::{self, ModifiedSet, PathClass, ReplayUnit, WorkspaceGraph};
use frust_drive::hotpatch::jump_table::create_jump_table;
use frust_drive::hotpatch::layout::{self, LayoutTable};
use frust_drive::hotpatch::link_intercept::{LinkAction, LinkMode, read_link_args};
use frust_drive::hotpatch::replay::{replay_env, replay_units};
use frust_drive::hotpatch::seams::SeamSet;
use frust_drive::hotpatch::session::{
    AcceptedSets, RestartReason, check_debuginfo, fat_build_command,
};
use frust_drive::hotpatch::stub::create_undefined_symbol_stub;
use frust_drive::hotpatch::symbols::{ImageSymbols, SymbolCache, Target};
use frust_drive::hotpatch::thin_link::{self, ThinLinkRequest};
use frust_drive::hotpatch::{HotpatchError, hotpatch_root};
use frust_drive::process::{ProcessRunner, RealProcessRunner};
use frust_hotpatch::JumpTable;

/// The fixture's tip package, which is also its bin's name.
pub const TIP_PACKAGE: &str = "hotpatch-canary-app";

/// The fixture's driver package: a workspace member outside the tip's image.
const DRIVER_PACKAGE: &str = "hotpatch-canary-driver";

/// The layout gate's refusal of an input whose DWARF holds no type at all.
const NO_TYPE_INFORMATION: &str = "without type information";

/// The layout gate's refusal of an input with no DWARF at all.
const NO_DWARF: &str = "carries no DWARF";

/// A fat-built image and everything needed to patch it.
pub struct FatSession {
    runner: RealProcessRunner,
    graph: WorkspaceGraph,
    records: BTreeMap<RecordKey, RustcRecord>,
    modified: ModifiedSet,
    rlibs: BTreeMap<ReplayUnit, PathBuf>,
    tip_link_args: Vec<String>,
    tip_env: Vec<(String, String)>,
    tip_bin: ReplayUnit,
    tip_lib: ReplayUnit,
    cache: SymbolCache,
    target: Target,
    flavor: LinkerFlavor,
    linker: String,
    target_dir: PathBuf,
    session: String,
    crates: Vec<String>,
    accepted: AcceptedSets,
    exe: PathBuf,
}

/// What one change compiled to: the freshly replayed objects' layout table
/// and the seam instances of everything the patch would carry.
pub struct Candidate {
    pub replayed: Vec<ReplayUnit>,
    pub layouts: LayoutTable,
    pub seams: SeamSet,
}

/// A linked patch and the table that applies it.
pub struct LinkedPatch {
    pub path: PathBuf,
    pub bytes: u64,
    pub table: JumpTable,
}

/// The toolchain identity the canary ran against.
pub struct Toolchain {
    pub rustc_version: String,
    pub triple: String,
}

impl Toolchain {
    /// `rustc -vV` (as the capture scope keys it) and its host triple.
    pub fn detect() -> Result<Self> {
        let rustc_version = capture::rustc_version(&RealProcessRunner)?;
        let triple = rustc_version
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .map(|triple| triple.trim().to_string())
            .ok_or_else(|| anyhow!("`rustc -vV` reports no `host:` triple"))?;
        Ok(Self {
            rustc_version,
            triple,
        })
    }

    /// The first line of `rustc -vV`, its LLVM line, and the host triple.
    pub fn summary(&self) -> String {
        let release = self.rustc_version.lines().next().unwrap_or_default();
        let llvm = self
            .rustc_version
            .lines()
            .find(|line| line.starts_with("LLVM version"))
            .unwrap_or("LLVM version unknown");
        format!("{release}; {llvm}; host {}", self.triple)
    }
}

impl FatSession {
    /// Fat-builds the fixture at `root` with `driver_exe` as the rustc
    /// wrapper and linker stand-in, links the fat image, seeds the accepted
    /// sets and loads the symbol cache. `log` receives progress lines and
    /// the build's diagnostics.
    pub fn build(
        root: &Path,
        driver_exe: &Path,
        toolchain: &Toolchain,
        log: &mut dyn FnMut(&str),
    ) -> Result<Self> {
        let runner = RealProcessRunner;
        let metadata = graph::cargo_metadata(&runner, &root.join("Cargo.toml"), None)?;
        let mut graph = WorkspaceGraph::from_metadata(&metadata, TIP_PACKAGE, None)?;
        check_debuginfo(&RealEnv, root, graph.workspace_root())?;
        let target_dir = target_directory(&metadata)?;
        let tip_bin = graph.tip_bin();
        let tip_lib = graph
            .tip_lib()
            .ok_or_else(|| anyhow!("the fixture's tip package has no lib target"))?;

        let target = Target::from_triple(&toolchain.triple)?;
        let flavor = LinkerFlavor::for_triple(&toolchain.triple)?;
        let scope = ScopeInputs {
            tip: TIP_PACKAGE.to_string(),
            triple: toolchain.triple.clone(),
            profile: "dev".to_string(),
            features: Vec::new(),
            rustflags: rustflags(&RealEnv),
            rustc_version: toolchain.rustc_version.clone(),
        };
        let scope_dir = prepare_scope_dir(&target_dir, &scope)?;
        // The session busts every other member; the driver is not in the
        // tip's image, so it is left alone to keep its own build warm.
        let members: Vec<String> = graph
            .packages()
            .iter()
            .filter(|p| p.member && p.name != TIP_PACKAGE && p.name != DRIVER_PACKAGE)
            .map(|p| p.name.clone())
            .collect();
        capture::bust_fingerprints(
            &capture::fingerprint_dir(&target_dir, None, "dev"),
            &scope_dir,
            TIP_PACKAGE,
            &members,
        )?;

        let fat_dir = hotpatch_root(&target_dir)
            .join("fat")
            .join(scope.dir_name()?);
        std::fs::create_dir_all(&fat_dir)
            .with_context(|| format!("creating `{}`", fat_dir.display()))?;
        let link = LinkAction {
            mode: LinkMode::NoLink,
            args_file: fat_dir.join("link-args.json"),
            err_file: Some(fat_dir.join("link-err.txt")),
        };
        remove_stale(&link.args_file)?;
        let fat = fat_build_command(
            TIP_PACKAGE,
            &tip_bin.target,
            &[],
            &[],
            driver_exe,
            &scope_dir,
            &link,
        );
        log(&format!(
            "fat build: {} {} (capture scope {})",
            fat.program,
            fat.args.join(" "),
            scope.dir_name()?
        ));
        run_fat_build(&runner, &fat.program, &fat.args, &fat.env, root, log)
            .map_err(|err| with_link_report(err, link.err_file.as_deref()))?;

        let link_args = read_link_args(&link.args_file)?;
        let records = load_records(&scope_dir)?;
        let keys: Vec<String> = records.keys().map(ToString::to_string).collect();
        log(&format!("fat build: captured rustc records {keys:?}"));
        for unit in [&tip_bin, &tip_lib] {
            if !records.contains_key(&unit.record_key()) {
                bail!(
                    "the fat build captured no `{}` invocation",
                    unit.record_key()
                );
            }
        }
        let tip_record = &records[&tip_bin.record_key()];
        let tip_env = replay_env(tip_record);
        let linker =
            fat_link::linker_program(custom_linker(&RealEnv, &toolchain.triple).as_deref())?;
        let exe = fat_dir.join(&tip_bin.target);
        let fat_out = fat_link::fat_link(
            &runner,
            &FatLinkRequest {
                flavor,
                linker: &linker,
                link_args: &link_args,
                envs: &tip_env,
                target_dir: &target_dir,
                archive_dir: &fat_dir,
                exe: &exe,
            },
        )?;
        let tip_objects = tip_objects(&link_args);
        log(&format!(
            "fat link: `{}` via `{linker}` ({flavor:?}): {} captured link args, {} tip objects, \
             anchor at {:#x}",
            fat_out.exe.display(),
            link_args.len(),
            tip_objects.len(),
            fat_out.anchor_address
        ));

        let rlibs = member_rlibs(&link_args, &graph);
        let tip_rlib = rlibs.get(&tip_lib).ok_or_else(|| {
            anyhow!(
                "the captured fat link names no rlib for {tip_lib}; a patch could not replay it"
            )
        })?;
        log(&format!("fat link: tip lib rlib `{}`", tip_rlib.display()));
        let crates = replayable_crates(&graph);
        let typed = typed_objects(&tip_objects, &crates)?;
        let base_layouts = layout::extract(
            &rlibs.values().cloned().chain(typed).collect::<Vec<_>>(),
            &crates,
        )?
        .table;
        let base_seams = SeamSet::from_inputs(
            &rlibs
                .values()
                .cloned()
                .chain(tip_objects)
                .collect::<Vec<_>>(),
        )?;
        let session = format!("session-{}", tip_bin.target);
        let accepted = AcceptedSets::begin(&target_dir, &session, base_layouts, base_seams)?;
        let cache = SymbolCache::load(&fat_out.exe, target)?;
        if cache.anchor_address() != fat_out.anchor_address {
            bail!(
                "the symbol cache reads the anchor at {:#x}, the fat link reported {:#x}",
                cache.anchor_address(),
                fat_out.anchor_address
            );
        }
        seed_dep_info(&mut graph, &records);

        Ok(Self {
            runner,
            graph,
            records,
            modified: ModifiedSet::new(),
            rlibs,
            tip_link_args: link_args,
            tip_env,
            tip_bin,
            tip_lib,
            cache,
            target,
            flavor,
            linker,
            target_dir,
            session,
            crates,
            accepted,
            exe: fat_out.exe,
        })
    }

    /// The fat image to launch.
    pub fn exe(&self) -> &Path {
        &self.exe
    }

    /// The accepted-layout set (the base until a patch is accepted).
    pub fn accepted_layouts(&self) -> &LayoutTable {
        self.accepted.layouts()
    }

    pub fn accepted_seams(&self) -> &SeamSet {
        self.accepted.seams()
    }

    /// The thin build of one changed file: classify it, record the change,
    /// replay what it touches (libs only: the fixture's tip bin never
    /// changes), and read the candidate's layouts and seams.
    pub fn compile(&mut self, changed: &Path) -> Result<Candidate> {
        let mut class = self.graph.classify(changed);
        if class == PathClass::Unaffected
            && let Ok(canonical) = std::fs::canonicalize(changed)
        {
            class = self.graph.classify(&canonical);
        }
        let units = match class {
            PathClass::Replayable { units } => units,
            other => bail!(
                "`{}` classifies as {other:?}, not replayable",
                changed.display()
            ),
        };
        let plan = self.modified.record_change(&self.graph, &units)?;
        if plan.replay.contains(&self.tip_bin) {
            bail!("the change replays the tip bin; the canary edits the tip lib only");
        }
        if !plan.replay.contains(&self.tip_lib) {
            bail!(
                "the change to `{}` does not replay {} (plan {:?})",
                changed.display(),
                self.tip_lib,
                plan.replay
            );
        }
        let outcomes = replay_units(&self.runner, &self.graph, &self.records, &plan.replay)?;
        let mut fresh = Vec::new();
        for outcome in outcomes {
            if !outcome.success {
                bail!(
                    "replaying {} failed to compile:\n{}",
                    outcome.unit,
                    outcome.diagnostics.join("\n")
                );
            }
            let rlib = outcome.rlib()?.to_path_buf();
            if let Some(dep_info) = outcome.dep_info() {
                let cwd = self.graph.replay_cwd(&outcome.unit);
                if let Ok(files) = graph::read_dep_info(dep_info, &cwd) {
                    self.graph.set_dep_info(outcome.unit.clone(), files);
                }
            }
            self.rlibs.insert(outcome.unit.clone(), rlib.clone());
            fresh.push(rlib);
        }
        let layouts = layout::extract(&fresh, &self.crates)?.table;
        let seams = SeamSet::from_inputs(&self.patch_inputs()?)?;
        Ok(Candidate {
            replayed: plan.replay,
            layouts,
            seams,
        })
    }

    /// The host gates a session runs before sending anything: L3 against the
    /// accepted layouts, then component `State` identity.
    pub fn check(&self, candidate: &Candidate) -> Result<SeamSet, RestartReason> {
        self.accepted
            .check(&candidate.layouts, candidate.seams.clone())
    }

    /// Links patch `n` against the process whose anchor is at
    /// `anchor_runtime` and builds its jump table, mirrored into the
    /// runtime's [`JumpTable`] with the patch's path.
    pub fn link(&self, n: u32, anchor_runtime: u64) -> Result<LinkedPatch> {
        let inputs = self.patch_inputs()?;
        let stub = create_undefined_symbol_stub(&self.cache, &inputs, anchor_runtime)?;
        let session_dir = self.accepted.dir();
        let stub_object = session_dir.join(format!("stub-{n}.o"));
        std::fs::write(&stub_object, stub)
            .with_context(|| format!("writing `{}`", stub_object.display()))?;
        let output = thin_link::patch_path(&self.target_dir, &self.session, n, self.flavor)?;
        let rlibs = self.modified_rlibs()?;
        let linked = thin_link::thin_link(
            &self.runner,
            &ThinLinkRequest {
                flavor: self.flavor,
                linker: &self.linker,
                tip_link_args: &self.tip_link_args,
                replayed_rlibs: &rlibs,
                stub_object: &stub_object,
                output: &output,
                envs: &self.tip_env,
            },
        )?;
        let bytes = std::fs::read(&linked.patch)
            .with_context(|| format!("reading `{}`", linked.patch.display()))?;
        let symbols = ImageSymbols::parse(
            &bytes,
            self.target,
            &format!("patch `{}`", linked.patch.display()),
        )?;
        let wire = create_jump_table(&self.cache, &symbols)?;
        if wire.map.is_empty() {
            bail!("the jump table maps no function");
        }
        Ok(LinkedPatch {
            path: linked.patch.clone(),
            bytes: bytes.len() as u64,
            table: JumpTable {
                lib: linked.patch,
                map: wire.map.into_iter().collect(),
                aslr_reference: wire.aslr_reference,
                new_base_address: wire.new_base_address,
                ifunc_count: wire.ifunc_count,
            },
        })
    }

    /// Merges an applied candidate into the accepted sets.
    pub fn accept(&mut self, candidate: &Candidate, present: &SeamSet) -> Result<()> {
        self.accepted.accept(&candidate.layouts, present)?;
        Ok(())
    }

    /// Every modified lib's rlib, dependents before their dependencies.
    fn modified_rlibs(&self) -> Result<Vec<PathBuf>, HotpatchError> {
        let order = self.graph.replay_order(self.modified.units())?;
        order
            .iter()
            .rev()
            .filter(|unit| unit.kind == TargetKind::Lib)
            .map(|unit| {
                self.rlibs.get(unit).cloned().ok_or_else(|| {
                    HotpatchError::unsupported(format!("no rlib is known for {unit}"))
                })
            })
            .collect()
    }

    /// The objects a patch links: the tip's objects and every modified lib.
    fn patch_inputs(&self) -> Result<Vec<PathBuf>, HotpatchError> {
        let mut inputs = tip_objects(&self.tip_link_args);
        inputs.extend(self.modified_rlibs()?);
        Ok(inputs)
    }
}

/// Runs the fat build in `root`, forwarding cargo's rendered diagnostics and
/// any non-JSON line to `log`. A failed build carries its errors.
fn run_fat_build(
    runner: &RealProcessRunner,
    program: &str,
    args: &[String],
    env: &[(String, String)],
    root: &Path,
    log: &mut dyn FnMut(&str),
) -> Result<()> {
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut errors = Vec::new();
    let output = runner
        .run_streaming(
            program,
            &argv,
            Some(root),
            &env,
            &mut |line| match serde_json::from_str::<serde_json::Value>(line) {
                Ok(message) => {
                    if message.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
                        return;
                    }
                    let inner = message.get("message");
                    let Some(rendered) = inner
                        .and_then(|m| m.get("rendered"))
                        .and_then(|r| r.as_str())
                    else {
                        return;
                    };
                    log(rendered.trim_end());
                    if inner.and_then(|m| m.get("level")).and_then(|l| l.as_str()) == Some("error")
                    {
                        errors.push(rendered.to_string());
                    }
                }
                Err(_) => log(line),
            },
        )
        .context("spawning the fat build")?;
    if output.success {
        return Ok(());
    }
    let stderr = output.stderr.trim();
    if !stderr.is_empty() {
        errors.push(stderr.to_string());
    }
    bail!("the fat build failed:\n{}", errors.join("\n"))
}

/// Appends the intercepted linker's own report, when it wrote one.
fn with_link_report(err: anyhow::Error, err_file: Option<&Path>) -> anyhow::Error {
    match err_file.and_then(|file| std::fs::read_to_string(file).ok()) {
        Some(report) => err.context(format!("intercepted link step: {report}")),
        None => err,
    }
}

/// `cargo metadata`'s `target_directory`.
fn target_directory(metadata: &str) -> Result<PathBuf> {
    let value: serde_json::Value =
        serde_json::from_str(metadata).context("unreadable `cargo metadata` output")?;
    value
        .get("target_directory")
        .and_then(|dir| dir.as_str())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("`cargo metadata` names no target_directory"))
}

/// The rustflags cargo applies: `CARGO_ENCODED_RUSTFLAGS`, else `RUSTFLAGS`.
fn rustflags(env: &dyn EnvLookup) -> Vec<String> {
    if let Some(encoded) = env.get("CARGO_ENCODED_RUSTFLAGS") {
        return encoded
            .split('\u{1f}')
            .filter(|flag| !flag.is_empty())
            .map(str::to_string)
            .collect();
    }
    env.get("RUSTFLAGS")
        .map(|flags| flags.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// A `CARGO_TARGET_<TRIPLE>_LINKER` override for `triple`.
fn custom_linker(env: &dyn EnvLookup, triple: &str) -> Option<PathBuf> {
    let key = format!(
        "CARGO_TARGET_{}_LINKER",
        triple.to_ascii_uppercase().replace(['-', '.'], "_")
    );
    env.get(&key)
        .filter(|linker| !linker.is_empty())
        .map(PathBuf::from)
}

/// Removes a file a previous build left, so a stale one is never read.
fn remove_stale(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("removing `{}`", path.display())),
    }
}

/// The `.rcgu.o` objects a captured tip link names: the tip bin's own code.
fn tip_objects(link_args: &[String]) -> Vec<PathBuf> {
    link_args
        .iter()
        .filter(|arg| arg.ends_with(".rcgu.o"))
        .map(PathBuf::from)
        .collect()
}

/// The tip objects whose DWARF describes at least one type; at least one
/// object must carry DWARF at all.
fn typed_objects(objects: &[PathBuf], crates: &[String]) -> Result<Vec<PathBuf>, HotpatchError> {
    let mut typed = Vec::new();
    let mut with_dwarf = 0usize;
    for object in objects {
        match layout::extract(std::slice::from_ref(object), crates) {
            Ok(_) => {
                typed.push(object.clone());
                with_dwarf += 1;
            }
            Err(HotpatchError::BuilderUnsupported { detail })
                if detail.contains(NO_TYPE_INFORMATION) =>
            {
                with_dwarf += 1;
            }
            Err(HotpatchError::BuilderUnsupported { detail }) if detail.contains(NO_DWARF) => {}
            Err(err) => return Err(err),
        }
    }
    if !objects.is_empty() && with_dwarf == 0 {
        return Err(HotpatchError::unsupported(format!(
            "none of the tip's {} objects carries DWARF; the layout gate needs `debug = true`",
            objects.len()
        )));
    }
    Ok(typed)
}

/// The rlib a captured link names for each workspace-member lib in the
/// image: `lib<crate>.rlib` or `lib<crate>-<hash>.rlib`.
fn member_rlibs(link_args: &[String], graph: &WorkspaceGraph) -> BTreeMap<ReplayUnit, PathBuf> {
    let rlibs: Vec<(&str, &String)> = link_args
        .iter()
        .filter_map(|arg| {
            let name = Path::new(arg).file_name()?.to_str()?;
            let stem = name.strip_prefix("lib")?.strip_suffix(".rlib")?;
            Some((stem, arg))
        })
        .collect();
    graph
        .units()
        .into_iter()
        .filter(|unit| unit.kind == TargetKind::Lib)
        .filter_map(|unit| {
            let crate_name = unit.record_key().crate_name;
            let hashed = format!("{crate_name}-");
            let path = rlibs
                .iter()
                .find(|(stem, _)| *stem == crate_name || stem.starts_with(&hashed))?
                .1;
            Some((unit, PathBuf::from(path)))
        })
        .collect()
}

/// Every member target's crate name: the crates whose types L3 fingerprints.
fn replayable_crates(graph: &WorkspaceGraph) -> Vec<String> {
    let names: BTreeSet<String> = graph
        .units()
        .iter()
        .map(|unit| unit.record_key().crate_name)
        .collect();
    names.into_iter().collect()
}

/// Seeds the graph's dep-info from each captured compile's
/// `<out-dir>/<crate><extra-filename>.d`; a missing one is skipped.
fn seed_dep_info(graph: &mut WorkspaceGraph, records: &BTreeMap<RecordKey, RustcRecord>) {
    for unit in graph.units() {
        let Some(record) = records.get(&unit.record_key()) else {
            continue;
        };
        let args = record.rustc_args();
        let Some(out_dir) = flag_value(args, "--out-dir") else {
            continue;
        };
        let extra = codegen_value(args, "extra-filename").unwrap_or_default();
        let cwd = graph.replay_cwd(&unit);
        let file = cwd
            .join(out_dir)
            .join(format!("{}{extra}.d", unit.record_key().crate_name));
        if let Ok(files) = graph::read_dep_info(&file, &cwd) {
            graph.set_dep_info(unit, files);
        }
    }
}

/// The value of `flag` in its `flag value` or `flag=value` form.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let prefix = format!("{flag}=");
    args.iter()
        .enumerate()
        .find_map(|(i, arg)| match arg.strip_prefix(&prefix) {
            Some(value) => Some(value.to_string()),
            None if arg == flag => args.get(i + 1).cloned(),
            None => None,
        })
}

/// The value of codegen option `option` in its `-Coption=value` or
/// `-C option=value` form.
fn codegen_value(args: &[String], option: &str) -> Option<String> {
    let prefix = format!("{option}=");
    args.iter().enumerate().find_map(|(i, arg)| {
        let spec = match arg.strip_prefix("-C") {
            Some("") => args.get(i + 1)?.as_str(),
            Some(spec) => spec,
            None => return None,
        };
        spec.strip_prefix(&prefix).map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn tip_objects_are_the_rcgu_objects() {
        let args = strings(&["-o", "x", "/t/a.rcgu.o", "/t/libdep.rlib", "/t/b.rcgu.o"]);
        assert_eq!(
            tip_objects(&args),
            vec![PathBuf::from("/t/a.rcgu.o"), PathBuf::from("/t/b.rcgu.o")]
        );
    }

    #[test]
    fn flag_and_codegen_values_read_both_spellings() {
        let args = strings(&[
            "--out-dir",
            "/o",
            "-C",
            "extra-filename=-ab",
            "-Cdebuginfo=2",
        ]);
        assert_eq!(flag_value(&args, "--out-dir").as_deref(), Some("/o"));
        assert_eq!(
            codegen_value(&args, "extra-filename").as_deref(),
            Some("-ab")
        );
        assert_eq!(codegen_value(&args, "debuginfo").as_deref(), Some("2"));
        let joined = strings(&["--out-dir=/p"]);
        assert_eq!(flag_value(&joined, "--out-dir").as_deref(), Some("/p"));
    }

    #[test]
    fn the_metadata_target_directory_is_required() {
        assert_eq!(
            target_directory(r#"{"target_directory": "/t"}"#).unwrap(),
            PathBuf::from("/t")
        );
        assert!(target_directory("{}").is_err());
    }
}
