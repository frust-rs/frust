//! rustc-invocation capture: `frust` as cargo's `RUSTC_WORKSPACE_WRAPPER`.
//!
//! A fat build runs cargo with `RUSTC_WORKSPACE_WRAPPER=<frust>` and
//! [`CAPTURE_ENV`]`=<scope dir>`, so cargo calls `frust <rustc> <args...>`
//! for every workspace member. `frust-cli`'s `main` hands that invocation to
//! [`run_wrapper`] before any CLI parsing. Each compile is persisted as a
//! [`RustcRecord`] (`{args, envs, crate_types}`), then the real rustc runs
//! with its output passed through. rustc later invokes `frust` again as its
//! linker (`-Clinker=<frust>`, the environment inherited); that invocation
//! is recognised as a link step and handed to
//! [`link_intercept`](super::link_intercept).
//!
//! **Privacy.** A record holds the invocation's environment, so a scope
//! directory is created `0700` and a record `0600` from the moment it
//! exists (unix; no-op elsewhere), and a scope found less private (one an
//! older builder made) is tightened once, records included. A record keeps
//! three classes of variable and nothing else ([`env_is_recorded`]):
//!
//! 1. **What cargo injected**: every name absent from the environment the
//!    host handed cargo ([`AMBIENT_ENV`] lists those names) — `OUT_DIR`,
//!    a build script's `cargo:rustc-env`, a `.cargo/config` `[env]` entry.
//!    The replaying host never has these, so a record must; they are the
//!    build's own contract, and cargo already writes them world-readable
//!    under `target/` (`build/<pkg>/output`).
//! 2. **What the compile read**: the `# env-dep:` names of the unit's
//!    dep-info (`env!`/`option_env!`), added once rustc has succeeded, so
//!    an `option_env!` takes the same branch on replay. A proc macro's
//!    untracked `std::env::var` read is not in the dep-info; it reads the
//!    replaying host's environment, which is class 3's fallback anyway.
//! 3. **The allowlisted ambient build environment**
//!    ([`env_is_allowlisted`]): cargo (`CARGO*`), the build-script contract
//!    (`TARGET`, `HOST`, `PROFILE`, `OPT_LEVEL`, `DEBUG`, `NUM_JOBS`), the
//!    toolchain (`RUSTC*`, `RUSTFLAGS`, `RUSTDOC*`, `RUST_*`, `RUSTUP_*`,
//!    the `CC`/`CXX`/`AR`/`LD`/`RANLIB`/`*FLAGS` families and `*_LINKER`),
//!    `PATH`, `HOME`, temp dirs, locale, the Apple and Android SDK
//!    variables, `PKG_CONFIG_*` and the wrapper's own `FRUST_*` — minus any
//!    name that looks like a credential (`*TOKEN*`, `*SECRET*`,
//!    `*PASSWORD*`, `*CREDENTIAL*`, `*_KEY*`, e.g. `CARGO_REGISTRY_TOKEN`)
//!    and minus any value carrying URL userinfo (`https://user:pw@host`,
//!    e.g. an authenticated `CARGO_HTTP_PROXY`). Everything else ambient
//!    (an exported `GITHUB_TOKEN`, cloud keys) is dropped; a replay
//!    inherits the host's current environment for whatever is not recorded.
//!
//! **Record key.** A record is stored as `{crate}.bin.json` iff `bin` is
//! among the invocation's `--crate-type` values, else `{crate}.lib.json`,
//! and keeps every type. dioxus-cli 0.7.10 read only the first
//! `--crate-type` and filed `cdylib` as a bin, so a lib declared
//! `["cdylib", "staticlib", "rlib"]` was missing when looked up as a lib.
//!
//! **Scope.** Captures live under
//! `<target>/frust-hotpatch/.captured-args/<tip>-<triple>-<profile>-<hash16>`
//! ([`ScopeInputs`]); the hash is a fixed 64-bit FNV-1a over the profile,
//! the feature set, the rustflags and the rustc version, so a capture is
//! never replayed under a configuration it was not taken in. A fat build
//! busts cargo's fingerprints ([`bust_fingerprints`]) so the wrapper sees a
//! fresh compile wherever a record is due.
//!
//! **Path dependencies outside the workspace.** cargo applies
//! `RUSTC_WORKSPACE_WRAPPER` to workspace members only, so a local path
//! package that is not a member (a `--frust-path` checkout) needs
//! `RUSTC_WRAPPER`, which cargo applies to every crate. A session that
//! captures such packages writes their list ([`NonMembers`]) into the scope
//! as [`NON_MEMBERS_FILE`] before the fat build, keyed into the scope name
//! ([`ScopeInputs::dir_name_for`]); [`wrapper_env`] then adds
//! `RUSTC_WRAPPER`. Under both wrappers cargo runs `frust frust rustc ...`
//! for a member: the outer invocation records it with its program stripped
//! and runs rustc itself, so a member's record is the one a member-only
//! scope holds. Any other compile is recorded only when its
//! `CARGO_MANIFEST_DIR` is a listed package and it is a lib; everything
//! else (registry crates, build scripts) runs unchanged and unrecorded. A
//! pass-through `RUSTC_WRAPPER` changes no cargo fingerprint, so a cached
//! non-member never reaches the wrapper: the fat build busts each listed
//! package that has no record yet.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::host_path;
use crate::process::ProcessRunner;

use super::link_intercept::{self, LinkAction};
use super::{HotpatchError, hotpatch_root};

/// Set (non-empty) to the scope directory, this turns the `frust` binary
/// into the rustc wrapper / linker stand-in. Unset, `frust` is the CLI.
pub const CAPTURE_ENV: &str = "FRUST_HOTPATCH_CAPTURE";
/// cargo's wrapper variable, applied to workspace members only.
pub const WORKSPACE_WRAPPER_ENV: &str = "RUSTC_WORKSPACE_WRAPPER";
/// cargo's wrapper variable applied to every crate; set only for a scope
/// that captures local non-members ([`NON_MEMBERS_FILE`]).
pub const WRAPPER_ENV: &str = "RUSTC_WRAPPER";
/// The scope file listing the local non-member packages a fat build
/// captures ([`NonMembers`]). Its presence makes [`wrapper_env`] set
/// [`WRAPPER_ENV`].
pub const NON_MEMBERS_FILE: &str = "non-members.json";
/// The names of the environment the host handed cargo, newline-separated
/// ([`ambient_env_var`]). A wrapper invocation finds cargo's own additions
/// by their absence from this list. Unset, the wrapper records the
/// allowlisted families only.
pub const AMBIENT_ENV: &str = "FRUST_HOTPATCH_AMBIENT";
/// The directory under [`hotpatch_root`] that holds every capture scope.
pub const CAPTURED_ARGS_DIR: &str = ".captured-args";

/// The crate name cargo uses for its target-info probes, which are passed
/// through and never recorded.
const PROBE_CRATE_NAME: &str = "___";

/// Every `--crate-type` value rustc accepts.
const KNOWN_CRATE_TYPES: &[&str] = &[
    "bin",
    "lib",
    "rlib",
    "dylib",
    "cdylib",
    "staticlib",
    "proc-macro",
];

/// The scope directory [`CAPTURE_ENV`] names, read through `get`; `None`
/// when it is unset or empty.
pub fn wrapper_scope_from_lookup(get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    get(CAPTURE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// [`wrapper_scope_from_lookup`] over this process's environment.
pub fn wrapper_scope_from_env() -> Option<PathBuf> {
    wrapper_scope_from_lookup(|key| std::env::var_os(key))
}

/// Which kind of target a capture record describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TargetKind {
    Lib,
    Bin,
}

impl TargetKind {
    /// `Bin` iff `bin` is among `crate_types`, else `Lib`.
    pub fn of(crate_types: &[String]) -> Self {
        if crate_types.iter().any(|ty| ty == "bin") {
            Self::Bin
        } else {
            Self::Lib
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Self::Lib => "lib",
            Self::Bin => "bin",
        }
    }
}

/// A record's key, `{crate}.{lib|bin}` (rendered by `Display`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecordKey {
    pub crate_name: String,
    pub kind: TargetKind,
}

impl RecordKey {
    pub fn new(crate_name: impl Into<String>, crate_types: &[String]) -> Self {
        Self {
            crate_name: crate_name.into(),
            kind: TargetKind::of(crate_types),
        }
    }

    /// Parses `{crate}.lib` / `{crate}.bin`.
    pub fn parse(key: &str) -> Option<Self> {
        let (crate_name, suffix) = key.rsplit_once('.')?;
        let kind = match suffix {
            "lib" => TargetKind::Lib,
            "bin" => TargetKind::Bin,
            _ => return None,
        };
        (!crate_name.is_empty()).then(|| Self {
            crate_name: crate_name.to_string(),
            kind,
        })
    }

    /// `{crate}.{lib|bin}.json`.
    pub fn file_name(&self) -> String {
        format!("{self}.json")
    }
}

impl std::fmt::Display for RecordKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.crate_name, self.kind.suffix())
    }
}

/// One captured rustc invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustcRecord {
    /// The wrapper's arguments as cargo passed them: the rustc program
    /// first, then rustc's own arguments.
    pub args: Vec<String>,
    /// The invocation's recorded environment ([`env_is_recorded`], plus
    /// what the compile's dep-info says it read), sorted by name.
    pub envs: Vec<(String, String)>,
    /// Every `--crate-type` value, in order, deduplicated.
    pub crate_types: Vec<String>,
}

impl RustcRecord {
    /// The rustc program cargo invoked.
    pub fn rustc(&self) -> Option<&str> {
        self.args.first().map(String::as_str)
    }

    /// rustc's own arguments (program excluded).
    pub fn rustc_args(&self) -> &[String] {
        self.args.get(1..).unwrap_or_default()
    }

    pub fn kind(&self) -> TargetKind {
        TargetKind::of(&self.crate_types)
    }
}

/// What a wrapper invocation is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// A workspace-member compile, to be recorded and then run.
    Rustc {
        crate_name: String,
        crate_types: Vec<String>,
    },
    /// rustc calling `frust` as its linker.
    Link,
    /// Anything else (cargo's `-vV` and `--crate-name ___` probes): run the
    /// program unchanged, record nothing.
    Passthrough,
}

/// Classifies a wrapper invocation (`args` = everything after the `frust`
/// program name). A `--crate-name` makes it a rustc compile, whose crate
/// name and types must parse; otherwise `.o`/`-flavor` arguments, directly
/// or in an `@` response file, make it a link step.
pub fn classify(args: &[String]) -> Result<Invocation, HotpatchError> {
    let names = flag_values(args, "--crate-name")?;
    if let Some(first) = names.first() {
        if names.iter().any(|name| name != first) {
            return Err(HotpatchError::unsupported(format!(
                "rustc invocation names several crates: {names:?}"
            )));
        }
        if first == PROBE_CRATE_NAME {
            return Ok(Invocation::Passthrough);
        }
        if first.is_empty() {
            return Err(HotpatchError::unsupported(
                "rustc invocation has an empty --crate-name",
            ));
        }
        return Ok(Invocation::Rustc {
            crate_name: first.clone(),
            crate_types: crate_types(args)?,
        });
    }
    if link_intercept::has_link_indicators(args)? {
        return Ok(Invocation::Link);
    }
    Ok(Invocation::Passthrough)
}

/// Every `--crate-type` value of a rustc invocation, in order, deduplicated
/// (`--crate-type a`, `--crate-type=a` and comma lists `a,b` all count).
/// None at all, an empty value or a type rustc does not know is
/// [`HotpatchError::BuilderUnsupported`].
pub fn crate_types(args: &[String]) -> Result<Vec<String>, HotpatchError> {
    let mut types: Vec<String> = Vec::new();
    for value in flag_values(args, "--crate-type")? {
        for ty in value.split(',').map(str::trim) {
            if !KNOWN_CRATE_TYPES.contains(&ty) {
                return Err(HotpatchError::unsupported(format!(
                    "unknown --crate-type `{ty}` in `{value}`"
                )));
            }
            if !types.iter().any(|known| known == ty) {
                types.push(ty.to_string());
            }
        }
    }
    if types.is_empty() {
        return Err(HotpatchError::unsupported(
            "rustc invocation has a --crate-name but no --crate-type",
        ));
    }
    Ok(types)
}

/// The values of `flag` in both the `flag value` and `flag=value` forms. A
/// trailing `flag` with no value is [`HotpatchError::BuilderUnsupported`].
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

/// Writes `record` as `<scope_dir>/<key>.json`, atomically (a temporary
/// file renamed into place), creating `scope_dir`. Returns the path.
pub fn write_record(
    scope_dir: &Path,
    key: &RecordKey,
    record: &RustcRecord,
) -> Result<PathBuf, HotpatchError> {
    ensure_private_scope(scope_dir)?;
    let json = serde_json::to_vec(record).map_err(|err| {
        HotpatchError::unsupported(format!("cannot serialize the {key} record: {err}"))
    })?;
    let path = scope_dir.join(key.file_name());
    let tmp = scope_dir.join(format!(".{}.tmp-{}", key.file_name(), std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    write_private_file(&tmp, &json)
        .map_err(|err| HotpatchError::io(format!("writing `{}`", tmp.display()), err))?;
    std::fs::rename(&tmp, &path)
        .map_err(|err| HotpatchError::io(format!("moving the {key} record into place"), err))?;
    Ok(path)
}

/// Creates `scope_dir` (and parents) and makes it owner-only (`0700`). A
/// scope that is already `0700` is left alone: nothing inside it is
/// reachable by anyone else, whatever the records' own modes. Any other
/// mode (a fresh directory under the umask, or a scope an older builder
/// left `0755`) is tightened once, along with its `*.json` records
/// (`0600`) — so a build's many wrapper invocations do not each re-chmod
/// every record, and a record renamed away by a parallel invocation
/// mid-sweep is not an error. A no-op beyond `create_dir_all` off unix.
fn ensure_private_scope(scope_dir: &Path) -> Result<(), HotpatchError> {
    std::fs::create_dir_all(scope_dir).map_err(|err| {
        HotpatchError::io(
            format!("creating capture scope `{}`", scope_dir.display()),
            err,
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let io = |what: &str, path: &Path, err| {
            HotpatchError::io(format!("{what} `{}`", path.display()), err)
        };
        let mode = std::fs::metadata(scope_dir)
            .map_err(|err| io("inspecting capture scope", scope_dir, err))?
            .permissions()
            .mode()
            & 0o777;
        if mode == 0o700 {
            return Ok(());
        }
        std::fs::set_permissions(scope_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|err| io("restricting capture scope", scope_dir, err))?;
        let entries = std::fs::read_dir(scope_dir)
            .map_err(|err| io("listing capture scope", scope_dir, err))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && entry.file_type().is_ok_and(|kind| kind.is_file())
            {
                match std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => return Err(io("restricting capture record", &path, err)),
                }
            }
        }
    }
    Ok(())
}

/// Writes `bytes` to a new file at `path` that is `0600` from creation.
fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// Reads one record. Malformed JSON, an empty `args`, or a file name whose
/// kind disagrees with the stored crate types is
/// [`HotpatchError::BuilderUnsupported`].
pub fn read_record(path: &Path) -> Result<(RecordKey, RustcRecord), HotpatchError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let key = file_name
        .strip_suffix(".json")
        .and_then(RecordKey::parse)
        .ok_or_else(|| {
            HotpatchError::unsupported(format!("`{}` is not a capture record name", path.display()))
        })?;
    let bytes = std::fs::read(path)
        .map_err(|err| HotpatchError::io(format!("reading `{}`", path.display()), err))?;
    let record: RustcRecord = serde_json::from_slice(&bytes).map_err(|err| {
        HotpatchError::unsupported(format!(
            "malformed capture record `{}`: {err}",
            path.display()
        ))
    })?;
    if record.args.is_empty() || record.crate_types.is_empty() {
        return Err(HotpatchError::unsupported(format!(
            "capture record `{}` has no rustc program or no crate types",
            path.display()
        )));
    }
    if record.kind() != key.kind {
        return Err(HotpatchError::unsupported(format!(
            "capture record `{}` holds crate types {:?}, not a {}",
            path.display(),
            record.crate_types,
            key.kind.suffix()
        )));
    }
    Ok((key, record))
}

/// Every record in `scope_dir`, by key. A missing directory is empty.
pub fn load_records(scope_dir: &Path) -> Result<BTreeMap<RecordKey, RustcRecord>, HotpatchError> {
    let mut records = BTreeMap::new();
    let entries = match std::fs::read_dir(scope_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(records),
        Err(err) => {
            return Err(HotpatchError::io(
                format!("listing capture scope `{}`", scope_dir.display()),
                err,
            ));
        }
    };
    for entry in entries {
        let path = entry
            .map_err(|err| HotpatchError::io(format!("listing `{}`", scope_dir.display()), err))?
            .path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.ends_with(".lib.json") || name.ends_with(".bin.json") {
            let (key, record) = read_record(&path)?;
            records.insert(key, record);
        }
    }
    Ok(records)
}

/// Runs one wrapper invocation: `args` is everything after the `frust`
/// program name, `envs` the process environment. A compile is recorded
/// into `scope_dir` and then run, and once it has succeeded the record is
/// rewritten with whatever its dep-info says the compile read from the
/// environment ([`env_is_recorded`]); a link step runs the [`LinkAction`]
/// the environment selects; anything else runs unchanged. The child's
/// stdout and stderr are passed through to `out`/`err`. Returns whether
/// the step succeeded (a failed compile or link is `Ok(false)`); a builder
/// problem is an `Err`, which fails the build rather than recording a
/// guess.
///
/// In a scope with a [`NON_MEMBERS_FILE`] the invocation may be cargo's
/// `RUSTC_WRAPPER` (see the module doc): a compile is recorded only as a
/// member's (its program the workspace wrapper, which is stripped) or as a
/// listed non-member's lib, and runs unchanged and unrecorded otherwise.
pub fn run_wrapper(
    runner: &dyn ProcessRunner,
    scope_dir: &Path,
    args: &[OsString],
    envs: &[(OsString, OsString)],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<bool, HotpatchError> {
    let args = utf8_args(args)?;
    match classify(&args)? {
        Invocation::Rustc {
            crate_name,
            crate_types,
        } => {
            let envs = utf8_envs(envs)?;
            let args = match read_non_members(scope_dir)? {
                None => args,
                Some(non_members) => {
                    match non_member_scope_compile(&args, &envs, &crate_types, &non_members) {
                        Some(args) => args,
                        None => return run_program(runner, &args, out, err),
                    }
                }
            };
            let key = RecordKey::new(crate_name, &crate_types);
            let ambient = ambient_names(&envs);
            let mut record = RustcRecord {
                args: args.clone(),
                envs: envs
                    .iter()
                    .filter(|(name, value)| env_is_recorded(name, value, ambient.as_ref()))
                    .cloned()
                    .collect(),
                crate_types,
            };
            write_record(scope_dir, &key, &record)?;
            let ok = run_program(runner, &args, out, err)?;
            if ok
                && let Some(dep_info) = dep_info_path(&args)?
                && let Some(read) = read_env_deps(&dep_info)?
                && record.keep_read(&read, &envs)
            {
                write_record(scope_dir, &key, &record)?;
            }
            Ok(ok)
        }
        Invocation::Link => {
            let action = LinkAction::from_lookup(|key| {
                envs.iter()
                    .find(|(name, _)| name.to_str() == Some(key))
                    .and_then(|(_, value)| value.to_str().map(str::to_string))
            })?
            .ok_or_else(|| {
                HotpatchError::unsupported(format!(
                    "a link step reached the wrapper without {}",
                    link_intercept::ENV_LINK
                ))
            })?;
            link_intercept::run_link(runner, &action, &args, out, err)
        }
        Invocation::Passthrough => run_program(runner, &args, out, err),
    }
}

/// The rustc invocation a compile in a non-member scope records, or `None`
/// when it runs unrecorded. cargo nests the wrappers for a member
/// (`RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER rustc ...`), so a program equal
/// to the workspace wrapper marks a member: its invocation is the rest,
/// recorded and run here directly rather than through a second wrapper
/// process. Any other compile is a non-member's or a registry crate's; only
/// a lib of a listed package (by `CARGO_MANIFEST_DIR`) is recorded.
fn non_member_scope_compile(
    args: &[String],
    envs: &[(String, String)],
    crate_types: &[String],
    non_members: &NonMembers,
) -> Option<Vec<String>> {
    let env = |name: &str| {
        envs.iter()
            .find(|(set, _)| set == name)
            .map(|(_, value)| value.as_str())
    };
    if let (Some(program), Some(workspace_wrapper)) = (args.first(), env(WORKSPACE_WRAPPER_ENV))
        && !workspace_wrapper.is_empty()
        && program == workspace_wrapper
    {
        return Some(args[1..].to_vec());
    }
    let listed =
        env("CARGO_MANIFEST_DIR").is_some_and(|dir| non_members.contains_dir(Path::new(dir)));
    (listed && TargetKind::of(crate_types) == TargetKind::Lib).then(|| args.to_vec())
}

/// One local path package outside the workspace whose lib a fat build
/// captures through `RUSTC_WRAPPER`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NonMember {
    /// The package name, as `cargo metadata` spells it.
    pub name: String,
    /// The directory holding its `Cargo.toml`: cargo's `CARGO_MANIFEST_DIR`
    /// for its compiles.
    pub dir: PathBuf,
}

/// The local non-member packages a scope captures, sorted by name then
/// directory. Empty for a member-only scope, which then is exactly the
/// scope a session without them uses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonMembers {
    packages: Vec<NonMember>,
}

impl NonMembers {
    /// `packages`, sorted and deduplicated, directories lexically
    /// normalised.
    pub fn new(packages: impl IntoIterator<Item = NonMember>) -> Self {
        let set: BTreeSet<NonMember> = packages
            .into_iter()
            .map(|package| NonMember {
                dir: super::graph::normalize(&package.dir),
                ..package
            })
            .collect();
        Self {
            packages: set.into_iter().collect(),
        }
    }

    pub fn packages(&self) -> &[NonMember] {
        &self.packages
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// The package names, in order: the dependencies a fat build busts
    /// until each has a record ([`bust_fingerprints`]).
    pub fn names(&self) -> Vec<String> {
        self.packages.iter().map(|p| p.name.clone()).collect()
    }

    /// Whether `dir` (a `CARGO_MANIFEST_DIR`) is a listed package's,
    /// compared lexically normalised.
    pub fn contains_dir(&self, dir: &Path) -> bool {
        let dir = super::graph::normalize(dir);
        self.packages.iter().any(|package| package.dir == dir)
    }
}

/// Writes `non_members` as `<scope_dir>/`[`NON_MEMBERS_FILE`], atomically
/// and owner-only.
fn write_non_members(scope_dir: &Path, non_members: &NonMembers) -> Result<(), HotpatchError> {
    let json = serde_json::to_vec(non_members).map_err(|err| {
        HotpatchError::unsupported(format!("cannot serialize the non-member list: {err}"))
    })?;
    let path = scope_dir.join(NON_MEMBERS_FILE);
    let tmp = scope_dir.join(format!(".{NON_MEMBERS_FILE}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    write_private_file(&tmp, &json)
        .map_err(|err| HotpatchError::io(format!("writing `{}`", tmp.display()), err))?;
    std::fs::rename(&tmp, &path)
        .map_err(|err| HotpatchError::io(format!("moving `{}` into place", path.display()), err))
}

/// The scope's [`NON_MEMBERS_FILE`], `None` when the scope has none (a
/// member-only scope). An unreadable or malformed one is an error: the
/// wrapper must not guess which compiles to record.
pub fn read_non_members(scope_dir: &Path) -> Result<Option<NonMembers>, HotpatchError> {
    let path = scope_dir.join(NON_MEMBERS_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(HotpatchError::io(
                format!("reading `{}`", path.display()),
                err,
            ));
        }
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|err| {
        HotpatchError::unsupported(format!(
            "malformed non-member list `{}`: {err}",
            path.display()
        ))
    })
}

fn run_program(
    runner: &dyn ProcessRunner,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<bool, HotpatchError> {
    let (program, rest) = args.split_first().ok_or_else(|| {
        HotpatchError::unsupported("the rustc wrapper was invoked with no program")
    })?;
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    let output = runner
        .run(program, &rest)
        .map_err(|e| HotpatchError::Process {
            detail: format!("failed to spawn `{program}`: {e:#}"),
        })?;
    out.write_all(output.stdout.as_bytes())
        .and_then(|()| err.write_all(output.stderr.as_bytes()))
        .map_err(|e| HotpatchError::io(format!("forwarding `{program}` output"), e))?;
    Ok(output.success)
}

fn utf8_args(args: &[OsString]) -> Result<Vec<String>, HotpatchError> {
    args.iter()
        .map(|arg| {
            arg.to_str().map(str::to_string).ok_or_else(|| {
                HotpatchError::unsupported(format!("non-UTF-8 wrapper argument {arg:?}"))
            })
        })
        .collect()
}

/// Exact names a record keeps beyond the prefix families.
const ENV_ALLOW_EXACT: &[&str] = &[
    "CARGO",
    "OUT_DIR",
    "TARGET",
    "HOST",
    "PROFILE",
    "OPT_LEVEL",
    "DEBUG",
    "NUM_JOBS",
    "RUSTFLAGS",
    "PATH",
    "HOME",
    "TMPDIR",
    "TEMP",
    "TMP",
    "LANG",
    "SDKROOT",
    "DEVELOPER_DIR",
    "MACOSX_DEPLOYMENT_TARGET",
    "IPHONEOS_DEPLOYMENT_TARGET",
    "CC",
    "CXX",
    "AR",
    "LD",
    "RANLIB",
    "CFLAGS",
    "CXXFLAGS",
    "LDFLAGS",
];

/// Prefixes a record keeps (`CARGO_*`, `RUSTC*`, `CC_<triple>`, ...).
const ENV_ALLOW_PREFIX: &[&str] = &[
    "CARGO_",
    "RUSTC",
    "RUSTDOC",
    "RUST_",
    "RUSTUP_",
    "CC_",
    "CXX_",
    "AR_",
    "LD_",
    "RANLIB_",
    "CFLAGS_",
    "CXXFLAGS_",
    "LDFLAGS_",
    "LC_",
    "ANDROID_",
    "NDK_",
    "PKG_CONFIG_",
    "FRUST_",
];

/// Substrings that mark a credential; they win over the allowlist.
const ENV_DENY_SUBSTRING: &[&str] = &["TOKEN", "SECRET", "PASSWORD", "CREDENTIAL", "_KEY"];

/// Whether an *ambient* variable `name` is in the allowlisted build
/// environment and not credential-shaped (class 3 of the module doc).
pub fn env_is_allowlisted(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if ENV_DENY_SUBSTRING.iter().any(|bad| upper.contains(bad)) {
        return false;
    }
    ENV_ALLOW_EXACT.contains(&name)
        || ENV_ALLOW_PREFIX
            .iter()
            .any(|prefix| name.starts_with(prefix))
        || name.ends_with("_LINKER")
}

/// Whether a record keeps `name=value` before the compile has run (the
/// module doc's classes 1 and 3; class 2 is [`RustcRecord::keep_read`]).
/// With `ambient` known, a name absent from it was injected by cargo and
/// is kept whatever it looks like; an ambient name is kept only when
/// [`env_is_allowlisted`] and its value carries no URL userinfo. Without
/// an ambient list every name is judged as ambient. The ambient list
/// itself is the wrapper's input, never a compile's environment.
pub fn env_is_recorded(name: &str, value: &str, ambient: Option<&BTreeSet<String>>) -> bool {
    if name == AMBIENT_ENV {
        return false;
    }
    match ambient {
        Some(ambient) if !ambient.contains(name) => true,
        _ => env_is_allowlisted(name) && !has_url_userinfo(value),
    }
}

/// Whether `value` holds a URL whose authority carries userinfo
/// (`scheme://user:password@host`), checked for every `://` in it.
pub fn has_url_userinfo(value: &str) -> bool {
    let mut rest = value;
    while let Some(at) = rest.find("://") {
        let authority = &rest[at + 3..];
        let end = authority.find(['/', '?', '#']).unwrap_or(authority.len());
        if authority[..end].contains('@') {
            return true;
        }
        rest = &authority[end..];
    }
    false
}

/// The ambient-name set [`AMBIENT_ENV`] carries in `envs`, if set.
fn ambient_names(envs: &[(String, String)]) -> Option<BTreeSet<String>> {
    envs.iter()
        .find(|(name, _)| name == AMBIENT_ENV)
        .map(|(_, value)| value.lines().map(str::to_string).collect())
}

/// The [`AMBIENT_ENV`] pair for an environment made of `names` (the host's
/// own plus whatever it adds for cargo): sorted, deduplicated,
/// newline-separated.
pub fn ambient_env_var<'a>(names: impl IntoIterator<Item = &'a str>) -> (String, String) {
    let names: BTreeSet<&str> = names.into_iter().collect();
    (
        AMBIENT_ENV.to_string(),
        names.into_iter().collect::<Vec<_>>().join("\n"),
    )
}

/// This process's environment names (non-UTF-8 names skipped: the wrapper
/// refuses those anyway).
pub fn ambient_env_names() -> Vec<String> {
    std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .collect()
}

impl RustcRecord {
    /// Adds every variable of `read` (the names the compile's dep-info
    /// says it read) that `envs` — the compile's full environment — holds
    /// and the record does not yet, keeping the record sorted. Returns
    /// whether anything was added. A name read but unset (an `option_env!`
    /// that took `None`) has nothing to add.
    pub fn keep_read(&mut self, read: &BTreeSet<String>, envs: &[(String, String)]) -> bool {
        let mut added = false;
        for name in read {
            if self.envs.iter().any(|(kept, _)| kept == name) {
                continue;
            }
            if let Some(pair) = envs.iter().find(|(set, _)| set == name) {
                self.envs.push(pair.clone());
                added = true;
            }
        }
        if added {
            self.envs.sort();
        }
        added
    }
}

/// Where the compile `args` describe will write its dep-info, or `None`
/// when `--emit` does not ask for one: an explicit `dep-info=<path>`, else
/// `<out-dir>/<crate-name><extra-filename>.d` (rustc's default naming, the
/// one cargo relies on), relative to the compile's working directory when
/// there is no `--out-dir`.
fn dep_info_path(args: &[String]) -> Result<Option<PathBuf>, HotpatchError> {
    let mut wanted = false;
    for value in flag_values(args, "--emit")? {
        for item in value.split(',').map(str::trim) {
            if let Some(path) = item.strip_prefix("dep-info=") {
                return Ok(Some(PathBuf::from(path)));
            }
            wanted |= item == "dep-info";
        }
    }
    if !wanted {
        return Ok(None);
    }
    let out_dir = flag_values(args, "--out-dir")?.pop().unwrap_or_default();
    let crate_name = flag_values(args, "--crate-name")?.pop().unwrap_or_default();
    let extra = codegen_values(args)
        .into_iter()
        .filter_map(|option| option.strip_prefix("extra-filename=").map(str::to_string))
        .next_back()
        .unwrap_or_default();
    Ok(Some(
        PathBuf::from(out_dir).join(format!("{crate_name}{extra}.d")),
    ))
}

/// Every `-C`/`--codegen` option value (`-C opt=val`, `-Copt=val`,
/// `--codegen opt=val`, `--codegen=opt=val`), in order.
fn codegen_values(args: &[String]) -> Vec<String> {
    let mut values = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-C" || arg == "--codegen" {
            if let Some(value) = iter.next() {
                values.push(value.clone());
            }
        } else if let Some(value) = arg
            .strip_prefix("--codegen=")
            .or_else(|| arg.strip_prefix("-C"))
        {
            values.push(value.to_string());
        }
    }
    values
}

/// The `# env-dep:` names of the dep-info at `path`
/// ([`parse_dep_info_env`](super::graph::parse_dep_info_env)). `None` when
/// the file does not exist (a compile that wrote its outputs elsewhere);
/// any other read failure is an error, since the record would otherwise
/// silently miss what the compile read.
fn read_env_deps(path: &Path) -> Result<Option<BTreeSet<String>>, HotpatchError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(super::graph::parse_dep_info_env(&text))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(HotpatchError::io(
            format!("reading dep-info `{}`", path.display()),
            err,
        )),
    }
}

fn utf8_envs(envs: &[(OsString, OsString)]) -> Result<Vec<(String, String)>, HotpatchError> {
    let mut pairs = envs
        .iter()
        .map(|(name, value)| match (name.to_str(), value.to_str()) {
            (Some(name), Some(value)) => Ok((name.to_string(), value.to_string())),
            _ => Err(HotpatchError::unsupported(format!(
                "non-UTF-8 environment variable {name:?}"
            ))),
        })
        .collect::<Result<Vec<_>, _>>()?;
    pairs.sort();
    Ok(pairs)
}

/// What a capture scope is keyed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeInputs {
    /// The tip crate (hyphens are normalised to rustc's underscores).
    pub tip: String,
    pub triple: String,
    /// The cargo profile name (`dev`, `release`, a custom profile).
    pub profile: String,
    /// Enabled features; order-insensitive.
    pub features: Vec<String>,
    /// rustflags; order-sensitive, a later flag can override an earlier.
    pub rustflags: Vec<String>,
    /// `rustc -vV` output ([`rustc_version`]).
    pub rustc_version: String,
}

/// Bumped whenever the hashed encoding or the record format changes, so an
/// older capture never matches.
const SCOPE_FORMAT_VERSION: u8 = 1;

impl ScopeInputs {
    /// 16 lowercase hex digits: FNV-1a 64 over the profile, the sorted
    /// feature set, the rustflags and the rustc version, each field and item
    /// length-prefixed so no two inputs share an encoding.
    pub fn hash16(&self) -> String {
        self.hash16_for(&NonMembers::default())
    }

    /// [`hash16`](Self::hash16) of a scope that also captures
    /// `non_members`: their names and directories are one more field, which
    /// an empty list leaves out, so a member-only scope keeps its hash and a
    /// non-member scope never shares one with it.
    pub fn hash16_for(&self, non_members: &NonMembers) -> String {
        let mut features = self.features.clone();
        features.sort();
        features.dedup();
        let mut hasher = Fnv1a64::new();
        hasher.write(&[SCOPE_FORMAT_VERSION]);
        hasher.field(b'p', std::slice::from_ref(&self.profile));
        hasher.field(b'f', &features);
        hasher.field(b'r', &self.rustflags);
        hasher.field(b'v', std::slice::from_ref(&self.rustc_version));
        if !non_members.is_empty() {
            let items: Vec<String> = non_members
                .packages()
                .iter()
                .flat_map(|p| [p.name.clone(), p.dir.to_string_lossy().into_owned()])
                .collect();
            hasher.field(b'n', &items);
        }
        format!("{:016x}", hasher.finish())
    }

    /// `<tip>-<triple>-<profile>-<hash16>`. A tip, triple or profile that is
    /// empty or could leave the directory is
    /// [`HotpatchError::BuilderUnsupported`].
    pub fn dir_name(&self) -> Result<String, HotpatchError> {
        self.dir_name_for(&NonMembers::default())
    }

    /// [`dir_name`](Self::dir_name) with [`hash16_for`](Self::hash16_for).
    pub fn dir_name_for(&self, non_members: &NonMembers) -> Result<String, HotpatchError> {
        let tip = self.tip.replace('-', "_");
        for (what, part) in [
            ("tip", tip.as_str()),
            ("triple", &self.triple),
            ("profile", &self.profile),
        ] {
            if part.is_empty() || part == "." || part == ".." || part.contains(['/', '\\']) {
                return Err(HotpatchError::unsupported(format!(
                    "capture scope {what} `{part}` is not a plain name"
                )));
            }
        }
        Ok(format!(
            "{tip}-{}-{}-{}",
            self.triple,
            self.profile,
            self.hash16_for(non_members)
        ))
    }
}

/// The fixed 64-bit FNV-1a hash, used instead of a cryptographic digest:
/// a scope name only has to separate configurations, and the value must be
/// stable across releases and hosts (unlike `std`'s `DefaultHasher`).
struct Fnv1a64(u64);

impl Fnv1a64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn field(&mut self, tag: u8, items: &[String]) {
        self.write(&[tag]);
        self.write(&(items.len() as u64).to_le_bytes());
        for item in items {
            self.write(&(item.len() as u64).to_le_bytes());
            self.write(item.as_bytes());
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// `<target_dir>/frust-hotpatch/.captured-args`.
pub fn captured_args_root(target_dir: &Path) -> PathBuf {
    hotpatch_root(target_dir).join(CAPTURED_ARGS_DIR)
}

/// The scope directory `inputs` select under `target_dir`.
pub fn scope_dir(target_dir: &Path, inputs: &ScopeInputs) -> Result<PathBuf, HotpatchError> {
    Ok(captured_args_root(target_dir).join(inputs.dir_name()?))
}

/// Creates the scope directory and returns its canonical path with any
/// Windows verbatim prefix removed ([`host_path::canonicalize_simplified`]),
/// the form passed to the wrapper through [`CAPTURE_ENV`].
pub fn prepare_scope_dir(
    target_dir: &Path,
    inputs: &ScopeInputs,
) -> Result<PathBuf, HotpatchError> {
    prepare_scope_dir_for(target_dir, inputs, &NonMembers::default())
}

/// [`prepare_scope_dir`] for a scope that also captures `non_members`: the
/// directory is named by [`ScopeInputs::dir_name_for`], and a non-empty
/// list is written into it as [`NON_MEMBERS_FILE`], which turns on
/// `RUSTC_WRAPPER` ([`wrapper_env`]) and the wrapper's filter.
pub fn prepare_scope_dir_for(
    target_dir: &Path,
    inputs: &ScopeInputs,
    non_members: &NonMembers,
) -> Result<PathBuf, HotpatchError> {
    let dir = captured_args_root(target_dir).join(inputs.dir_name_for(non_members)?);
    ensure_private_scope(&dir)?;
    if !non_members.is_empty() {
        write_non_members(&dir, non_members)?;
    }
    host_path::canonicalize_simplified(&dir).map_err(|err| {
        HotpatchError::io(format!("resolving capture scope `{}`", dir.display()), err)
    })
}

/// The `frust` executable to name as wrapper and linker.
pub fn frust_exe() -> Result<PathBuf, HotpatchError> {
    std::env::current_exe()
        .map(|exe| host_path::simplify(&exe))
        .map_err(|err| HotpatchError::io("locating the frust executable", err))
}

/// What a fat build tells the wrapper: the `frust` executable to name as
/// wrapper and linker, the scope to record into, and the names of the
/// host's own environment ([`ambient_env_names`]), from which the wrapper
/// tells cargo's additions apart.
#[derive(Debug, Clone, Copy)]
pub struct WrapperSetup<'a> {
    pub frust_exe: &'a Path,
    pub scope_dir: &'a Path,
    pub ambient_names: &'a [String],
}

/// The environment that makes cargo route workspace-member compiles through
/// `frust_exe`, recording into `scope_dir` — plus [`WRAPPER_ENV`], routing
/// every other compile through it too, when the scope holds a
/// [`NON_MEMBERS_FILE`] ([`prepare_scope_dir_for`]).
pub fn wrapper_env(frust_exe: &Path, scope_dir: &Path) -> Vec<(String, String)> {
    let render = |path: &Path| host_path::simplify(path).to_string_lossy().into_owned();
    let mut env = vec![
        (WORKSPACE_WRAPPER_ENV.to_string(), render(frust_exe)),
        (CAPTURE_ENV.to_string(), render(scope_dir)),
    ];
    if scope_dir.join(NON_MEMBERS_FILE).is_file() {
        env.push((WRAPPER_ENV.to_string(), render(frust_exe)));
    }
    env
}

/// `rustc -vV`, trimmed: the toolchain identity a scope is keyed on.
pub fn rustc_version(runner: &dyn ProcessRunner) -> Result<String, HotpatchError> {
    let output = runner
        .run("rustc", &["-vV"])
        .map_err(|e| HotpatchError::Process {
            detail: format!("failed to spawn `rustc -vV`: {e:#}"),
        })?;
    let version = output.stdout.trim();
    if !output.success || !version.starts_with("rustc ") {
        return Err(HotpatchError::unsupported(format!(
            "unexpected `rustc -vV` output: {}{}",
            output.stdout, output.stderr
        )));
    }
    Ok(version.to_string())
}

/// cargo's fingerprint directory for a build: `<target>/<triple>/<dir>`
/// with an explicit `--target`, `<target>/<dir>` without, where `<dir>` is
/// `debug` for the `dev`/`test` profiles, `release` for `release`/`bench`,
/// else the profile's own name.
pub fn fingerprint_dir(target_dir: &Path, triple: Option<&str>, profile: &str) -> PathBuf {
    let profile_dir = match profile {
        "dev" | "test" => "debug",
        "release" | "bench" => "release",
        other => other,
    };
    let base = match triple {
        Some(triple) => target_dir.join(triple),
        None => target_dir.to_path_buf(),
    };
    base.join(profile_dir).join(".fingerprint")
}

/// Forces a fat build to recompile (and so re-capture) what it must: the
/// tip package always — its link step has to reach the interception again —
/// and every local dependency in `workspace_deps` (the other members, and
/// any [`NonMembers`] the scope captures) that has no `{dep}.lib.json`
/// record in `scope_dir` yet. Removes each matching
/// `<package>-<hash>` entry of `fingerprint_dir` (names compared with
/// hyphens normalised to underscores) and returns the removed paths. A
/// missing fingerprint directory (a clean target) busts nothing.
///
/// This relies on cargo's fingerprint layout, which is not a stable
/// interface; it stops working if cargo moves to content-based
/// fingerprints.
pub fn bust_fingerprints(
    fingerprint_dir: &Path,
    scope_dir: &Path,
    tip: &str,
    workspace_deps: &[String],
) -> Result<Vec<PathBuf>, HotpatchError> {
    let normalise = |name: &str| name.replace('-', "_");
    let mut bust = vec![normalise(tip)];
    for dep in workspace_deps {
        let dep = normalise(dep);
        let key = RecordKey {
            crate_name: dep.clone(),
            kind: TargetKind::Lib,
        };
        if !scope_dir.join(key.file_name()).exists() && !bust.contains(&dep) {
            bust.push(dep);
        }
    }
    let entries = match std::fs::read_dir(fingerprint_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(HotpatchError::io(
                format!("listing `{}`", fingerprint_dir.display()),
                err,
            ));
        }
    };
    let mut removed = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|err| {
                HotpatchError::io(format!("listing `{}`", fingerprint_dir.display()), err)
            })?
            .path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((package, _hash)) = name.rsplit_once('-') else {
            continue;
        };
        if bust.contains(&normalise(package)) {
            std::fs::remove_dir_all(&path).map_err(|err| {
                HotpatchError::io(format!("busting fingerprint `{}`", path.display()), err)
            })?;
            removed.push(path);
        }
    }
    removed.sort();
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotpatch::link_intercept::{ENV_ARGS_FILE, ENV_LINK};
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-capture-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    fn envs(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    fn unsupported<T: std::fmt::Debug>(result: Result<T, HotpatchError>) -> String {
        match result {
            Err(HotpatchError::BuilderUnsupported { detail }) => detail,
            other => panic!("expected BuilderUnsupported, got {other:?}"),
        }
    }

    fn ok_output(stderr: &str) -> Output {
        Output {
            success: true,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    /// The template lib's compile, as cargo passes it to the wrapper.
    const LIB_COMPILE: &[&str] = &[
        "/rust/bin/rustc",
        "--crate-name",
        "my_app",
        "--edition=2024",
        "src/lib.rs",
        "--crate-type",
        "cdylib",
        "--crate-type",
        "staticlib",
        "--crate-type",
        "rlib",
        "--emit=dep-info,metadata,link",
    ];

    #[test]
    fn a_cdylib_staticlib_rlib_compile_is_keyed_lib_with_every_type_stored() {
        let scope = temp_dir("lib-key");
        let runner = FakeProcessRunner::new()
            .with(LIB_COMPILE.join(" "), ok_output("{\"artifact\":\"x\"}\n"));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let env = envs(&[
            ("CARGO_PKG_NAME", "my-app"),
            ("CARGO_MANIFEST_DIR", "/w/my-app"),
        ]);
        let ok = run_wrapper(&runner, &scope, &os(LIB_COMPILE), &env, &mut out, &mut err).unwrap();
        assert!(ok);
        assert_eq!(String::from_utf8(err).unwrap(), "{\"artifact\":\"x\"}\n");

        let records = load_records(&scope).unwrap();
        let key = RecordKey::parse("my_app.lib").unwrap();
        assert_eq!(records.keys().collect::<Vec<_>>(), vec![&key]);
        let record = &records[&key];
        assert_eq!(
            record.crate_types,
            strings(&["cdylib", "staticlib", "rlib"])
        );
        assert_eq!(record.args, strings(LIB_COMPILE));
        assert_eq!(record.rustc(), Some("/rust/bin/rustc"));
        assert_eq!(record.rustc_args()[0], "--crate-name");
        assert_eq!(
            record.envs,
            vec![
                ("CARGO_MANIFEST_DIR".to_string(), "/w/my-app".to_string()),
                ("CARGO_PKG_NAME".to_string(), "my-app".to_string()),
            ]
        );
        assert!(scope.join("my_app.lib.json").is_file());
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn a_new_scope_is_0700_and_its_record_0600() {
        let scope = temp_dir("private").join("nested").join("scope");
        let key = RecordKey::new("a", &strings(&["lib"]));
        let record = RustcRecord {
            args: strings(&["rustc"]),
            envs: Vec::new(),
            crate_types: strings(&["lib"]),
        };
        let path = write_record(&scope, &key, &record).unwrap();
        assert_eq!(mode(&scope), 0o700);
        assert_eq!(mode(&path), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn reusing_an_older_scope_tightens_it_and_its_records() {
        use std::os::unix::fs::PermissionsExt as _;
        let scope = temp_dir("reuse");
        fs::set_permissions(&scope, fs::Permissions::from_mode(0o755)).unwrap();
        let old = scope.join("old.lib.json");
        fs::write(&old, "{}").unwrap();
        fs::set_permissions(&old, fs::Permissions::from_mode(0o644)).unwrap();
        let key = RecordKey::new("new", &strings(&["lib"]));
        let record = RustcRecord {
            args: strings(&["rustc"]),
            envs: Vec::new(),
            crate_types: strings(&["lib"]),
        };
        write_record(&scope, &key, &record).unwrap();
        assert_eq!(mode(&scope), 0o700);
        assert_eq!(mode(&old), 0o600);
        assert_eq!(mode(&scope.join("new.lib.json")), 0o600);
    }

    #[test]
    fn secrets_never_reach_the_record_but_the_build_environment_does() {
        let scope = temp_dir("allowlist");
        let runner = FakeProcessRunner::new()
            .with(LIB_COMPILE.join(" "), ok_output("{\"artifact\":\"x\"}\n"));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let env = envs(&[
            ("SOME_SECRET_TOKEN", "x"),
            ("AWS_SECRET_ACCESS_KEY", "y"),
            ("GITHUB_TOKEN", "z"),
            ("CARGO_REGISTRY_TOKEN", "w"),
            ("CARGO_PKG_NAME", "my-app"),
            ("OUT_DIR", "/o"),
            ("PATH", "/bin"),
            ("FRUST_HOTPATCH_LINK", "1"),
            ("CC_aarch64_linux_android", "clang"),
            ("CARGO_TARGET_X_LINKER", "ld"),
        ]);
        run_wrapper(&runner, &scope, &os(LIB_COMPILE), &env, &mut out, &mut err).unwrap();
        let records = load_records(&scope).unwrap();
        let names: Vec<&str> = records
            .values()
            .next()
            .unwrap()
            .envs
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        for gone in [
            "SOME_SECRET_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
            "CARGO_REGISTRY_TOKEN",
        ] {
            assert!(!names.contains(&gone), "{gone} leaked: {names:?}");
        }
        for kept in [
            "CARGO_PKG_NAME",
            "OUT_DIR",
            "PATH",
            "FRUST_HOTPATCH_LINK",
            "CC_aarch64_linux_android",
            "CARGO_TARGET_X_LINKER",
        ] {
            assert!(names.contains(&kept), "{kept} dropped: {names:?}");
        }
    }

    /// The recorded names of the one record in `scope`.
    fn recorded_names(scope: &Path) -> Vec<String> {
        let records = load_records(scope).unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        records
            .values()
            .next()
            .unwrap()
            .envs
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    #[test]
    fn what_cargo_injected_is_kept_whatever_it_is_named_and_the_ambient_list_is_not() {
        let scope = temp_dir("injected");
        let runner = FakeProcessRunner::new()
            .with(LIB_COMPILE.join(" "), ok_output("{\"artifact\":\"x\"}\n"));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        // The host handed cargo these names (its own shell plus the fat
        // build's additions); everything else below is cargo's doing.
        let ambient = ambient_env_var([
            "PATH",
            "HOME",
            "GITHUB_TOKEN",
            "CARGO_REGISTRY_TOKEN",
            "RUSTC_WORKSPACE_WRAPPER",
            "FRUST_HOTPATCH_CAPTURE",
            AMBIENT_ENV,
        ]);
        let env = envs(&[
            ("PATH", "/bin"),
            ("HOME", "/home/dev"),
            ("GITHUB_TOKEN", "ambient-secret"),
            ("CARGO_REGISTRY_TOKEN", "ambient-secret"),
            ("RUSTC_WORKSPACE_WRAPPER", "/bin/frust"),
            ("FRUST_HOTPATCH_CAPTURE", "/t/scope"),
            (&ambient.0, &ambient.1),
            // cargo's additions: the standard ones, a build script's
            // `cargo:rustc-env` (credential-shaped or not) and a
            // `.cargo/config` `[env]` entry.
            ("CARGO_PKG_NAME", "my-app"),
            ("OUT_DIR", "/t/out"),
            ("APP_API_KEY", "from-build-rs"),
            ("APP_VERSION_STAMP", "2026-10-08"),
            ("DATABASE_URL", "postgres://u:p@db/app"),
        ]);
        run_wrapper(&runner, &scope, &os(LIB_COMPILE), &env, &mut out, &mut err).unwrap();
        let names = recorded_names(&scope);
        for kept in [
            "PATH",
            "HOME",
            "RUSTC_WORKSPACE_WRAPPER",
            "FRUST_HOTPATCH_CAPTURE",
            "CARGO_PKG_NAME",
            "OUT_DIR",
            "APP_API_KEY",
            "APP_VERSION_STAMP",
            "DATABASE_URL",
        ] {
            assert!(names.iter().any(|n| n == kept), "{kept} dropped: {names:?}");
        }
        for gone in ["GITHUB_TOKEN", "CARGO_REGISTRY_TOKEN", AMBIENT_ENV] {
            assert!(!names.iter().any(|n| n == gone), "{gone} leaked: {names:?}");
        }
    }

    #[test]
    fn a_successful_compile_adds_the_variables_its_dep_info_says_it_read() {
        let scope = temp_dir("env-dep");
        let out_dir = temp_dir("env-dep-out");
        let compile: Vec<String> = LIB_COMPILE
            .iter()
            .map(|arg| arg.to_string())
            .chain([
                "-C".to_string(),
                "extra-filename=-abc".to_string(),
                "--out-dir".to_string(),
                out_dir.to_string_lossy().into_owned(),
            ])
            .collect();
        // What the (fake) rustc wrote: a stamp read through `env!`, an
        // `option_env!` that found nothing, and a cargo variable.
        fs::write(
            out_dir.join("my_app-abc.d"),
            "x.d: src/lib.rs\n\n# env-dep:APP_STAMP=1\n# env-dep:APP_MISSING\n# env-dep:CARGO_PKG_NAME=my-app\n",
        )
        .unwrap();
        let env = envs(&[
            ("APP_STAMP", "1"),
            ("APP_UNREAD", "2"),
            ("CARGO_PKG_NAME", "my-app"),
        ]);
        let compile_os: Vec<OsString> = compile.iter().map(OsString::from).collect();

        // A failed compile leaves the pre-run record: nothing was read.
        let failing = FakeProcessRunner::new().with(
            compile.join(" "),
            Output {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert!(!run_wrapper(&failing, &scope, &compile_os, &env, &mut out, &mut err).unwrap());
        assert_eq!(recorded_names(&scope), vec!["CARGO_PKG_NAME"]);

        let runner = FakeProcessRunner::new().with(compile.join(" "), ok_output(""));
        assert!(run_wrapper(&runner, &scope, &compile_os, &env, &mut out, &mut err).unwrap());
        assert_eq!(recorded_names(&scope), vec!["APP_STAMP", "CARGO_PKG_NAME"]);
    }

    #[test]
    fn dep_info_is_located_the_way_rustc_names_it() {
        let located = |args: &[&str]| dep_info_path(&strings(args)).unwrap();
        assert_eq!(
            located(&["rustc", "--crate-name", "a", "--emit=link"]),
            None
        );
        assert_eq!(
            located(&[
                "rustc",
                "--crate-name",
                "a",
                "--emit=dep-info,link",
                "-C",
                "extra-filename=-1",
                "--out-dir",
                "/t/deps"
            ]),
            Some(PathBuf::from("/t/deps/a-1.d"))
        );
        assert_eq!(
            located(&[
                "rustc",
                "--crate-name",
                "a",
                "--emit",
                "dep-info=/t/custom.d,link",
                "--out-dir",
                "/t/deps"
            ]),
            Some(PathBuf::from("/t/custom.d"))
        );
        assert_eq!(
            located(&[
                "rustc",
                "--crate-name",
                "a",
                "--emit=dep-info",
                "-Cextra-filename=-2",
                "--codegen=opt-level=0",
                "--out-dir=/t/deps"
            ]),
            Some(PathBuf::from("/t/deps/a-2.d"))
        );
        assert_eq!(
            located(&["rustc", "--crate-name", "a", "--emit=dep-info"]),
            Some(PathBuf::from("a.d")),
            "no --out-dir: rustc writes beside its working directory"
        );
    }

    #[test]
    fn authenticated_urls_never_reach_the_record_but_plain_ones_do() {
        assert!(has_url_userinfo("http://user:pw@proxy:3128"));
        assert!(has_url_userinfo("https://token@host/index"));
        assert!(has_url_userinfo("a=http://x/ b=https://u:p@h/"));
        assert!(!has_url_userinfo("http://proxy:3128"));
        assert!(!has_url_userinfo("https://host/path?user@domain"));
        assert!(!has_url_userinfo("git@github.com:org/repo"));
        assert!(!has_url_userinfo(""));

        let scope = temp_dir("userinfo");
        let runner = FakeProcessRunner::new()
            .with(LIB_COMPILE.join(" "), ok_output("{\"artifact\":\"x\"}\n"));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let env = envs(&[
            ("CARGO_HTTP_PROXY", "http://user:pw@proxy:3128"),
            ("CARGO_REGISTRIES_MINE_INDEX", "https://host/git/index"),
            ("CARGO_PKG_NAME", "my-app"),
        ]);
        run_wrapper(&runner, &scope, &os(LIB_COMPILE), &env, &mut out, &mut err).unwrap();
        assert_eq!(
            recorded_names(&scope),
            vec!["CARGO_PKG_NAME", "CARGO_REGISTRIES_MINE_INDEX"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_already_private_scope_is_not_swept_again() {
        use std::os::unix::fs::PermissionsExt as _;
        let scope = temp_dir("no-resweep");
        fs::set_permissions(&scope, fs::Permissions::from_mode(0o700)).unwrap();
        // Unreachable through the 0700 scope whatever its own mode; the
        // sweep is the one-time pass over a scope found less private.
        let old = scope.join("old.lib.json");
        fs::write(&old, "{}").unwrap();
        fs::set_permissions(&old, fs::Permissions::from_mode(0o644)).unwrap();
        let key = RecordKey::new("new", &strings(&["lib"]));
        let record = RustcRecord {
            args: strings(&["rustc"]),
            envs: Vec::new(),
            crate_types: strings(&["lib"]),
        };
        write_record(&scope, &key, &record).unwrap();
        assert_eq!(mode(&scope), 0o700);
        assert_eq!(mode(&old), 0o644);
        assert_eq!(mode(&scope.join("new.lib.json")), 0o600);
    }

    #[test]
    fn bin_among_the_crate_types_keys_the_record_bin() {
        let lib_first = strings(&[
            "rustc",
            "--crate-name",
            "x",
            "--crate-type",
            "rlib",
            "--crate-type",
            "bin",
        ]);
        let Invocation::Rustc {
            crate_name,
            crate_types,
        } = classify(&lib_first).unwrap()
        else {
            panic!("a compile");
        };
        assert_eq!(
            RecordKey::new(crate_name, &crate_types).to_string(),
            "x.bin"
        );

        let scope = temp_dir("bin-key");
        let compile = [
            "/rust/bin/rustc",
            "--crate-name",
            "my_app",
            "src/main.rs",
            "--crate-type",
            "bin",
        ];
        let runner = FakeProcessRunner::new().with(compile.join(" "), ok_output(""));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        run_wrapper(&runner, &scope, &os(&compile), &[], &mut out, &mut err).unwrap();
        let records = load_records(&scope).unwrap();
        let key = RecordKey::parse("my_app.bin").unwrap();
        assert_eq!(records[&key].crate_types, strings(&["bin"]));
    }

    #[test]
    fn lib_and_bin_of_one_package_do_not_overwrite_each_other() {
        let scope = temp_dir("both");
        let bin = [
            "/rust/bin/rustc",
            "--crate-name",
            "my_app",
            "src/main.rs",
            "--crate-type",
            "bin",
        ];
        let runner = FakeProcessRunner::new()
            .with(LIB_COMPILE.join(" "), ok_output(""))
            .with(bin.join(" "), ok_output(""));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        run_wrapper(&runner, &scope, &os(LIB_COMPILE), &[], &mut out, &mut err).unwrap();
        run_wrapper(&runner, &scope, &os(&bin), &[], &mut out, &mut err).unwrap();
        let keys: Vec<String> = load_records(&scope)
            .unwrap()
            .keys()
            .map(ToString::to_string)
            .collect();
        assert_eq!(keys, vec!["my_app.lib", "my_app.bin"]);
    }

    #[test]
    fn crate_types_accept_equals_and_comma_forms_and_deduplicate() {
        let args = strings(&[
            "--crate-type=cdylib,rlib",
            "--crate-type",
            "rlib",
            "--crate-type",
            "staticlib",
        ]);
        assert_eq!(
            crate_types(&args).unwrap(),
            strings(&["cdylib", "rlib", "staticlib"])
        );
        assert_eq!(
            TargetKind::of(&crate_types(&args).unwrap()),
            TargetKind::Lib
        );
        assert_eq!(
            TargetKind::of(&crate_types(&strings(&["--crate-type", "proc-macro"])).unwrap()),
            TargetKind::Lib
        );
    }

    #[test]
    fn a_failed_compile_is_recorded_and_reported_as_a_failed_step() {
        let scope = temp_dir("fail");
        let runner = FakeProcessRunner::new().with(
            LIB_COMPILE.join(" "),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error[E0425]\n".to_string(),
            },
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert!(!run_wrapper(&runner, &scope, &os(LIB_COMPILE), &[], &mut out, &mut err).unwrap());
        assert_eq!(String::from_utf8(err).unwrap(), "error[E0425]\n");
        assert!(scope.join("my_app.lib.json").is_file());
    }

    #[test]
    fn probes_pass_through_unrecorded() {
        let scope = temp_dir("probe");
        let probe = [
            "/rust/bin/rustc",
            "-",
            "--crate-name",
            "___",
            "--print=file-names",
            "--crate-type",
            "bin",
        ];
        let runner = FakeProcessRunner::new()
            .with(
                probe.join(" "),
                Output {
                    success: true,
                    stdout: "lib___.rlib\n".to_string(),
                    stderr: String::new(),
                },
            )
            .with(
                "/rust/bin/rustc -vV",
                Output {
                    success: true,
                    stdout: "rustc 1.98.1\n".to_string(),
                    stderr: String::new(),
                },
            );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert!(run_wrapper(&runner, &scope, &os(&probe), &[], &mut out, &mut err).unwrap());
        assert!(
            run_wrapper(
                &runner,
                &scope,
                &os(&["/rust/bin/rustc", "-vV"]),
                &[],
                &mut out,
                &mut err
            )
            .unwrap()
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "lib___.rlib\nrustc 1.98.1\n"
        );
        assert!(load_records(&scope).unwrap().is_empty());
    }

    #[test]
    fn a_link_step_through_the_wrapper_runs_the_selected_link_action() {
        let scope = temp_dir("link");
        let args_file = scope.join("link-args.json");
        let exe = scope.join("out").join("my_app");
        let exe_text = exe.display().to_string();
        let link = [
            "-arch",
            "arm64",
            "/t/my_app.my_app.0.rcgu.o",
            "-o",
            exe_text.as_str(),
        ];
        let env = envs(&[
            (ENV_LINK, "no-link"),
            (ENV_ARGS_FILE, args_file.to_str().unwrap()),
            (CAPTURE_ENV, scope.to_str().unwrap()),
        ]);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert!(
            run_wrapper(
                &FakeProcessRunner::new(),
                &scope,
                &os(&link),
                &env,
                &mut out,
                &mut err
            )
            .unwrap()
        );
        assert_eq!(
            link_intercept::read_link_args(&args_file).unwrap(),
            strings(&link)
        );
        assert!(exe.is_file(), "the stand-in object is written at -o");
        assert!(
            load_records(&scope).unwrap().is_empty(),
            "a link step is not a compile record"
        );

        let no_action = run_wrapper(
            &FakeProcessRunner::new(),
            &scope,
            &os(&link),
            &[],
            &mut out,
            &mut err,
        );
        assert!(unsupported(no_action).contains(ENV_LINK));
    }

    #[test]
    fn malformed_invocations_are_builder_unsupported() {
        let scope = temp_dir("malformed");
        let runner = FakeProcessRunner::new().with("/rust/bin/rustc", ok_output(""));
        let mut sink = Vec::new();
        let mut run = |args: &[&str]| {
            let mut err = Vec::new();
            run_wrapper(&runner, &scope, &os(args), &[], &mut sink, &mut err)
        };
        assert!(
            unsupported(run(&[
                "/rust/bin/rustc",
                "--crate-name",
                "x",
                "--crate-type"
            ]))
            .contains("no value")
        );
        assert!(
            unsupported(run(&[
                "/rust/bin/rustc",
                "--crate-name",
                "x",
                "--crate-type",
                "wasm"
            ]))
            .contains("wasm")
        );
        assert!(
            unsupported(run(&["/rust/bin/rustc", "--crate-name", "x", "src/lib.rs"]))
                .contains("no --crate-type")
        );
        assert!(
            unsupported(run(&[
                "/rust/bin/rustc",
                "--crate-type",
                "lib",
                "--crate-name"
            ]))
            .contains("no value")
        );
        unsupported(run(&[
            "/rust/bin/rustc",
            "--crate-name",
            "a",
            "--crate-name",
            "b",
            "--crate-type",
            "lib",
        ]));
        unsupported(run(&[
            "/rust/bin/rustc",
            "--crate-name",
            "x",
            "--crate-type",
            ",",
        ]));
        unsupported(run(&[]));
        assert!(
            load_records(&scope).unwrap().is_empty(),
            "nothing is recorded on a refusal"
        );

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let bad = vec![
                OsString::from("/rust/bin/rustc"),
                OsString::from_vec(vec![0xff, 0xfe]),
            ];
            let mut err = Vec::new();
            unsupported(run_wrapper(&runner, &scope, &bad, &[], &mut sink, &mut err));
        }
    }

    #[test]
    fn malformed_records_are_builder_unsupported() {
        let scope = temp_dir("bad-record");
        fs::write(scope.join("x.lib.json"), "{\"args\": [\"rustc\"]").unwrap();
        unsupported(load_records(&scope));

        let scope = temp_dir("kind-mismatch");
        let record = RustcRecord {
            args: strings(&["rustc"]),
            envs: Vec::new(),
            crate_types: strings(&["bin"]),
        };
        fs::write(
            scope.join("x.lib.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert!(unsupported(load_records(&scope)).contains("not a lib"));

        let scope = temp_dir("empty-args");
        let record = RustcRecord {
            args: Vec::new(),
            envs: Vec::new(),
            crate_types: strings(&["rlib"]),
        };
        fs::write(
            scope.join("x.lib.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        unsupported(load_records(&scope));
    }

    #[test]
    fn records_round_trip_and_other_files_are_ignored() {
        let scope = temp_dir("roundtrip");
        let record = RustcRecord {
            args: strings(LIB_COMPILE),
            envs: vec![("A".to_string(), "1".to_string())],
            crate_types: strings(&["cdylib", "staticlib", "rlib"]),
        };
        let key = RecordKey::new("my_app", &record.crate_types);
        write_record(&scope, &key, &record).unwrap();
        fs::write(scope.join("link-args.json"), "[]").unwrap();
        let records = load_records(&scope).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[&key], record);
        assert!(load_records(&scope.join("missing")).unwrap().is_empty());
    }

    #[test]
    fn the_wrapper_is_selected_only_by_a_non_empty_capture_variable() {
        assert_eq!(wrapper_scope_from_lookup(|_| None), None);
        assert_eq!(wrapper_scope_from_lookup(|_| Some(OsString::new())), None);
        assert_eq!(
            wrapper_scope_from_lookup(
                |key| (key == CAPTURE_ENV).then(|| OsString::from("/t/scope"))
            ),
            Some(PathBuf::from("/t/scope"))
        );
        let env = wrapper_env(Path::new("/bin/frust"), Path::new("/t/scope"));
        assert_eq!(
            env,
            vec![
                (WORKSPACE_WRAPPER_ENV.to_string(), "/bin/frust".to_string()),
                (CAPTURE_ENV.to_string(), "/t/scope".to_string()),
            ]
        );
    }

    fn inputs() -> ScopeInputs {
        ScopeInputs {
            tip: "my-app".to_string(),
            triple: "aarch64-apple-darwin".to_string(),
            profile: "dev".to_string(),
            features: strings(&["frust/devtools", "frust/hotpatch"]),
            rustflags: strings(&["-Cdebuginfo=2"]),
            rustc_version: "rustc 1.98.1 (abc 2026-09-01)".to_string(),
        }
    }

    #[test]
    fn the_scope_hash_is_stable_for_equal_inputs() {
        let base = inputs();
        assert_eq!(base.hash16(), inputs().hash16());
        assert_eq!(base.hash16().len(), 16);
        assert!(
            base.hash16()
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );

        let mut reordered = inputs();
        reordered.features.reverse();
        reordered.features.push("frust/devtools".to_string());
        assert_eq!(
            reordered.hash16(),
            base.hash16(),
            "the feature set is order-insensitive"
        );

        // The tip and triple are in the name, not the hash.
        let mut renamed = inputs();
        renamed.tip = "other".to_string();
        assert_eq!(renamed.hash16(), base.hash16());
    }

    #[test]
    fn the_scope_hash_changes_with_each_keyed_input() {
        let base = inputs().hash16();
        type Change = fn(&mut ScopeInputs);
        let variants: [(&str, Change); 4] = [
            ("profile", |i| i.profile = "release".to_string()),
            ("features", |i| i.features = strings(&["frust/devtools"])),
            ("rustflags", |i| {
                i.rustflags.push("-Copt-level=1".to_string())
            }),
            ("rustc version", |i| {
                i.rustc_version = "rustc 1.99.0".to_string()
            }),
        ];
        for (what, change) in variants {
            let mut changed = inputs();
            change(&mut changed);
            assert_ne!(changed.hash16(), base, "{what} must change the hash");
        }
        let mut forward = inputs();
        forward.rustflags = strings(&["-Cdebuginfo=2", "-Copt-level=1"]);
        let mut backward = inputs();
        backward.rustflags = strings(&["-Copt-level=1", "-Cdebuginfo=2"]);
        assert_ne!(
            forward.hash16(),
            backward.hash16(),
            "rustflags order is significant"
        );
        // Length-prefixing: moving a byte between adjacent items changes it.
        let mut split = inputs();
        split.features = strings(&["ab", "c"]);
        let mut joined = inputs();
        joined.features = strings(&["a", "bc"]);
        assert_ne!(split.hash16(), joined.hash16());
    }

    #[test]
    fn the_scope_dir_is_named_tip_triple_profile_hash() {
        let target = Path::new("/w/build/rust");
        let dir = scope_dir(target, &inputs()).unwrap();
        let expected = format!("my_app-aarch64-apple-darwin-dev-{}", inputs().hash16());
        assert_eq!(
            dir,
            target
                .join("frust-hotpatch")
                .join(".captured-args")
                .join(expected)
        );
        let mut escaping = inputs();
        escaping.triple = "../x".to_string();
        unsupported(scope_dir(target, &escaping));
        let mut empty = inputs();
        empty.profile = String::new();
        unsupported(scope_dir(target, &empty));

        let tmp = temp_dir("prepare");
        let prepared = prepare_scope_dir(&tmp, &inputs()).unwrap();
        assert!(prepared.is_dir() && prepared.is_absolute());
    }

    #[test]
    fn rustc_version_reads_rustc_vv_and_refuses_anything_else() {
        let runner = FakeProcessRunner::new().with(
            "rustc -vV",
            Output {
                success: true,
                stdout: "rustc 1.98.1 (x)\nhost: aarch64-apple-darwin\n".to_string(),
                stderr: String::new(),
            },
        );
        assert_eq!(
            rustc_version(&runner).unwrap(),
            "rustc 1.98.1 (x)\nhost: aarch64-apple-darwin"
        );
        let odd = FakeProcessRunner::new().with("rustc -vV", ok_output(""));
        unsupported(rustc_version(&odd));
    }

    #[test]
    fn fingerprint_dir_follows_cargo_layout() {
        let t = Path::new("/t");
        assert_eq!(
            fingerprint_dir(t, None, "dev"),
            PathBuf::from("/t/debug/.fingerprint")
        );
        assert_eq!(
            fingerprint_dir(t, Some("aarch64-apple-darwin"), "release"),
            PathBuf::from("/t/aarch64-apple-darwin/release/.fingerprint")
        );
        assert_eq!(
            fingerprint_dir(t, None, "profiling"),
            PathBuf::from("/t/profiling/.fingerprint")
        );
    }

    #[test]
    fn a_fat_build_busts_the_tip_and_uncaptured_workspace_deps() {
        let root = temp_dir("bust");
        let fingerprints = root.join(".fingerprint");
        let scope = root.join("scope");
        for name in [
            "my-app-1111",
            "my-app-2222",
            "core-ui-3333",
            "captured-4444",
            "serde-5555",
        ] {
            fs::create_dir_all(fingerprints.join(name)).unwrap();
        }
        fs::create_dir_all(&scope).unwrap();
        fs::write(scope.join("captured.lib.json"), "{}").unwrap();

        let deps = strings(&["core-ui", "captured"]);
        let removed = bust_fingerprints(&fingerprints, &scope, "my-app", &deps).unwrap();
        let names: Vec<String> = removed
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["core-ui-3333", "my-app-1111", "my-app-2222"]);
        assert!(fingerprints.join("captured-4444").is_dir());
        assert!(
            fingerprints.join("serde-5555").is_dir(),
            "non-workspace crates are never busted"
        );

        assert!(
            bust_fingerprints(&root.join("absent"), &scope, "my-app", &deps)
                .unwrap()
                .is_empty()
        );
    }

    fn non_members() -> NonMembers {
        NonMembers::new([
            NonMember {
                name: "shared".to_string(),
                dir: PathBuf::from("/x/frust/crates/shared"),
            },
            NonMember {
                name: "frust-core".to_string(),
                dir: PathBuf::from("/x/frust/crates/./frust-core"),
            },
        ])
    }

    /// `rustc --crate-name <name> src/lib.rs --crate-type <ty>` as cargo
    /// passes it, `program` first.
    fn compile(program: &[&str], name: &str, ty: &str) -> Vec<String> {
        let mut args = strings(program);
        args.extend(strings(&[
            "/rust/bin/rustc",
            "--crate-name",
            name,
            "src/lib.rs",
            "--crate-type",
            ty,
        ]));
        args
    }

    #[test]
    fn the_non_member_list_keys_the_scope_and_turns_on_rustc_wrapper() {
        let empty = NonMembers::default();
        assert_eq!(inputs().hash16_for(&empty), inputs().hash16());
        assert_eq!(
            inputs().dir_name_for(&empty).unwrap(),
            inputs().dir_name().unwrap()
        );
        let listed = non_members();
        assert_ne!(inputs().hash16_for(&listed), inputs().hash16());
        let mut moved = listed.packages().to_vec();
        moved[0].dir = PathBuf::from("/y/frust-core");
        assert_ne!(
            inputs().hash16_for(&NonMembers::new(moved)),
            inputs().hash16_for(&listed),
            "a package's directory is keyed"
        );
        assert_eq!(
            listed.names(),
            strings(&["frust-core", "shared"]),
            "sorted by name"
        );

        let target = temp_dir("non-member-scope");
        let member_only = prepare_scope_dir(&target, &inputs()).unwrap();
        let scope = prepare_scope_dir_for(&target, &inputs(), &listed).unwrap();
        assert_ne!(scope, member_only);
        assert_eq!(read_non_members(&scope).unwrap(), Some(listed.clone()));
        assert_eq!(read_non_members(&member_only).unwrap(), None);
        assert!(
            load_records(&scope).unwrap().is_empty(),
            "the list is not a record"
        );
        let frust = Path::new("/bin/frust");
        assert_eq!(
            wrapper_env(frust, &member_only),
            vec![
                (WORKSPACE_WRAPPER_ENV.to_string(), "/bin/frust".to_string()),
                (
                    CAPTURE_ENV.to_string(),
                    member_only.to_string_lossy().into_owned()
                ),
            ],
            "a member-only scope sets no RUSTC_WRAPPER"
        );
        let env = wrapper_env(frust, &scope);
        assert_eq!(
            env.last(),
            Some(&(WRAPPER_ENV.to_string(), "/bin/frust".to_string()))
        );
        assert_eq!(env.len(), 3);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(scope.join(NON_MEMBERS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::write(scope.join(NON_MEMBERS_FILE), "not json").unwrap();
        unsupported(read_non_members(&scope));
    }

    #[test]
    fn a_non_member_scope_records_members_and_listed_libs_and_passes_the_rest_through() {
        let target = temp_dir("non-member-wrapper");
        let scope = prepare_scope_dir_for(&target, &inputs(), &non_members()).unwrap();
        let wrapper = ("RUSTC_WORKSPACE_WRAPPER", "/bin/frust");
        let member = compile(&["/bin/frust"], "my_app", "lib");
        let listed = compile(&[], "shared", "lib");
        let listed_build_script = compile(&[], "build_script_build", "bin");
        let registry = compile(&[], "serde", "lib");
        // Every compile runs rustc itself: the member's without a second
        // wrapper process.
        let runner = [&member[1..], &listed, &listed_build_script, &registry]
            .into_iter()
            .fold(FakeProcessRunner::new(), |runner, args| {
                runner.with(args.join(" "), ok_output(""))
            });
        let run = |args: &[String], dir: &str| {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let env = envs(&[wrapper, ("CARGO_MANIFEST_DIR", dir)]);
            let (mut out, mut err) = (Vec::new(), Vec::new());
            run_wrapper(&runner, &scope, &os(&args), &env, &mut out, &mut err).unwrap()
        };
        assert!(run(&member, "/w/my-app"));
        assert!(run(&listed, "/x/frust/crates/shared/"));
        assert!(run(&listed_build_script, "/x/frust/crates/shared"));
        assert!(run(&registry, "/home/.cargo/registry/src/serde-1.0.0"));

        let records = load_records(&scope).unwrap();
        let keys: Vec<String> = records.keys().map(ToString::to_string).collect();
        assert_eq!(keys, strings(&["my_app.lib", "shared.lib"]));
        let my_app = &records[&RecordKey::parse("my_app.lib").unwrap()];
        assert_eq!(
            my_app.args,
            member[1..].to_vec(),
            "a member's record is a member-only scope's"
        );
        assert_eq!(my_app.rustc(), Some("/rust/bin/rustc"));
        assert_eq!(
            records[&RecordKey::parse("shared.lib").unwrap()].args,
            listed
        );
    }

    #[test]
    fn a_member_only_scope_records_whatever_the_workspace_wrapper_passes() {
        let scope = temp_dir("member-only-wrapper");
        let registry = compile(&[], "serde", "lib");
        let runner = FakeProcessRunner::new().with(registry.join(" "), ok_output(""));
        let args: Vec<&str> = registry.iter().map(String::as_str).collect();
        let env = envs(&[("CARGO_MANIFEST_DIR", "/anywhere")]);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert!(run_wrapper(&runner, &scope, &os(&args), &env, &mut out, &mut err).unwrap());
        assert_eq!(
            load_records(&scope).unwrap().keys().collect::<Vec<_>>(),
            vec![&RecordKey::parse("serde.lib").unwrap()],
            "no list: cargo's workspace wrapper already filtered to members"
        );
    }

    #[test]
    fn a_fat_build_busts_uncaptured_non_members_like_members() {
        let root = temp_dir("bust-non-members");
        let fingerprints = root.join(".fingerprint");
        let scope = root.join("scope");
        for name in ["my-app-1", "shared-2", "frust-core-3", "serde-4"] {
            fs::create_dir_all(fingerprints.join(name)).unwrap();
        }
        fs::create_dir_all(&scope).unwrap();
        fs::write(scope.join("frust_core.lib.json"), "{}").unwrap();
        let removed =
            bust_fingerprints(&fingerprints, &scope, "my-app", &non_members().names()).unwrap();
        let names: Vec<String> = removed
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["my-app-1", "shared-2"]);
    }
}
