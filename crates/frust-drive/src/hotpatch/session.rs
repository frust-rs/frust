//! The hot-patch session: one running desktop app process, built fat, with
//! every later source change either offered to it as a patch or answered
//! with a restart.
//!
//! **Start** ([`start_desktop`], Debug builds only). The dev profile's
//! resolved debuginfo is checked first ([`check_debuginfo`]): the layout
//! gate reads DWARF types, so anything short of full debug info fails
//! closed before any build. The fat build is then `cargo rustc` with the
//! Debug hot-session features, `-Csave-temps=true -Clink-dead-code
//! -Clinker=<frust>`, `frust` as the workspace wrapper and the link step
//! intercepted ([`fat_build_command`]); the builder links the fat image
//! itself, writes `layout-base.json` and seeds the accepted-layout and
//! accepted-seam sets from it ([`AcceptedSets`]), builds the symbol cache,
//! spawns that image directly (never `cargo run`), reads the devtools
//! discovery line from its output and connects with the token. An app that
//! does not advertise `HotPatch`, or whose endpoint is not loopback, gives a
//! restart-only session carrying the reason.
//!
//! **Change** ([`HotSession::on_change`]). Changed paths are classified: a
//! file of a path dependency outside the workspace or a build input is a
//! restart with no thin build. Otherwise the changed units are replayed, the
//! candidate's layout table and seam instances are checked against the
//! accepted sets, `hotpatch_info`'s pending layout-mismatch records and the
//! patch budget are read, and only then is the stub built against the
//! process's runtime anchor, the patch thin-linked, uploaded in chunks and
//! applied. One patch is in flight at a time, and nothing is ever sent to a
//! restart-only session, to a non-loopback endpoint, with pending records
//! or over budget.
//!
//! **Accepted sets.** A candidate's layout and seam entries are merged only
//! when its `PatchOutcome` reports `applied: true` with no layout mismatch;
//! each merge rewrites `layouts-accepted.json` in the session dir for
//! diagnosis (never read back). Every start resets the session dir and
//! reseeds the sets from the new base, so a relaunch is a new session.
//!
//! **Outcome.** A reply lost after `apply_patch` was sent is
//! `PatchOutcomeUnknown` (the patch may be live). A missed seam key in the
//! newest patch is a benign patch-image caller; one in the base or an older
//! patch is mapped through that image's symbol table and requires a restart
//! when it names a seam instance the patch carries. No seam hit at all is a
//! restart too. Once a session has answered a restart, it answers the same
//! restart to every later change and sends nothing more. See
//! `docs/CLI_ARCHITECTURE.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use frust_devtools_protocol::{
    ApplyPatchParams, Capability, Discovery, JumpTableWire, MissedKey, PatchOutcome,
    parse_discovery_line, parse_failure_line, redact_discovery_token,
};

use crate::build_info::{BuildInfo, BuildMode};
use crate::desktop_run;
use crate::devtools_client::{DevtoolsClient, DevtoolsRpcError, is_not_supported};
use crate::doctor::{EnvLookup, RealEnv};
use crate::manifest::{self, HotpatchSection};
use crate::process::{ProcessRunner, StreamHandle, TryRecvError};

use super::capture::{
    self, CAPTURE_ENV, RecordKey, RustcRecord, ScopeInputs, TargetKind, load_records,
    prepare_scope_dir,
};
use super::fat_link::{self, FatLinkRequest, LinkerFlavor};
use super::graph::{PathClass, ReplayUnit, WorkspaceGraph};
use super::layout::{self, LayoutTable};
use super::link_intercept::{LinkAction, LinkMode, linker_arg, read_link_args};
use super::replay::{STRIPPED_ENV, parse_notifications, replay_args, replay_env, replay_units};
use super::seams::{self, SeamSet};
use super::symbols::{ImageSymbols, SymbolCache, Target};
use super::thin_link::{self, ThinLinkRequest};
use super::{HotpatchError, hotpatch_root};

/// The accepted-layout set after each merge, written beside
/// `layout-base.json` for diagnosis and never read back.
pub const LAYOUTS_ACCEPTED_FILE: &str = "layouts-accepted.json";

/// cargo's environment override of the dev profile's `debug` setting.
pub const DEV_DEBUG_ENV: &str = "CARGO_PROFILE_DEV_DEBUG";

/// The default patch budget: patches one process may load.
pub const DEFAULT_BUDGET_PATCHES: u32 = 64;

/// The default patch budget: patch-image bytes one process may load. With
/// patch images never unloaded, 64 patches at the measured ~0.55 MB of RSS
/// each keep a long session under +40 MB; the byte bound catches the
/// unusually large patch first.
pub const DEFAULT_BUDGET_BYTES: u64 = 96 * 1024 * 1024;

/// The connection timeout, and the bound every devtools call but
/// `apply_patch` waits for its answer.
const DEVTOOLS_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a freshly spawned fat image has to print its discovery line.
const DISCOVERY_DEADLINE: Duration = Duration::from_secs(60);

/// How often the discovery wait polls the child's output.
const DISCOVERY_POLL: Duration = Duration::from_millis(20);

/// `<target>/frust-hotpatch/fat/<scope>`: the fat image, its archive and
/// its captured link arguments, one directory per capture scope.
const FAT_DIR: &str = "fat";

/// How many patches a session may load before it must restart the app.
/// Patch images are never unloaded, so the budget bounds a long session's
/// memory growth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub patches: u32,
    pub bytes: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            patches: DEFAULT_BUDGET_PATCHES,
            bytes: DEFAULT_BUDGET_BYTES,
        }
    }
}

impl Budget {
    /// The default budget with each key `frust.toml`'s `[hotpatch]` section
    /// sets taking its place.
    pub fn from_section(section: Option<&HotpatchSection>) -> Self {
        let default = Self::default();
        Self {
            patches: section.and_then(|s| s.patches).unwrap_or(default.patches),
            bytes: section.and_then(|s| s.bytes).unwrap_or(default.bytes),
        }
    }

    /// `Some(PatchBudget)` when a process holding `patches` patches of
    /// `bytes` bytes would be over this budget.
    fn exceeded(&self, patches: u32, bytes: u64) -> Option<RestartReason> {
        (patches > self.patches || bytes > self.bytes).then_some(RestartReason::PatchBudget {
            patches,
            bytes,
            budget: *self,
        })
    }
}

/// Why a change cannot be hot-patched and the app must be relaunched. Its
/// `Display` is the text a front-end shows verbatim after `restart
/// required: `.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartReason {
    /// A file of a path package outside the workspace changed; only a fat
    /// rebuild captures such a package.
    PathDependencyChanged { package: String, file: PathBuf },
    /// A manifest, build script, cargo config or other build input changed.
    BuildInputChanged {
        package: Option<String>,
        file: PathBuf,
    },
    /// A type's layout differs from the accepted one: from the host's DWARF
    /// gate before sending, or from the app's own records.
    LayoutChanged { records: Vec<String> },
    /// A component's `State` type changed identity; the patch could not
    /// reach it.
    StateTypeChanged { changes: Vec<String> },
    /// The app does not offer hot patching: the capability is absent, the
    /// devtools service did not start or the connection could not be made.
    HotPatchUnavailable { reason: String },
    /// The devtools endpoint is not a loopback address.
    EndpointNotLoopback { endpoint: SocketAddr },
    /// The next patch would take the process past its budget: `patches`
    /// and `bytes` are the totals it would reach.
    PatchBudget {
        patches: u32,
        bytes: u64,
        budget: Budget,
    },
    /// `apply_patch` was sent and no answer arrived (closed connection,
    /// timeout, undecodable reply): the patch may be live.
    PatchOutcomeUnknown { detail: String },
    /// The app answered `apply_patch` with an error, or `applied: false`
    /// without layout records.
    PatchRefused { detail: String },
    /// A devtools call before `apply_patch` failed; nothing was applied.
    DevtoolsFailed { detail: String },
    /// Code in the base image (`image` 0) or an older patch still called a
    /// seam instance this patch replaced.
    StaleSeamCall { image: u32, symbol: String },
    /// A missed seam key could not be mapped to a symbol of a known image.
    UnresolvedMissedKey { image: u32, link_address: u64 },
    /// The patch was applied but no component's seam ran it.
    NoSeamHit,
    /// The builder met something it does not understand, or the build is
    /// not one it can patch.
    BuilderUnsupported { detail: String },
}

impl RestartReason {
    fn builder(err: &HotpatchError) -> Self {
        match err {
            HotpatchError::BuilderUnsupported { detail } => Self::BuilderUnsupported {
                detail: detail.clone(),
            },
            other => Self::BuilderUnsupported {
                detail: other.to_string(),
            },
        }
    }
}

impl fmt::Display for RestartReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PathDependencyChanged { package, file } => write!(
                f,
                "`{}` belongs to the path dependency `{package}`, which a patch cannot replay",
                file.display()
            ),
            Self::BuildInputChanged { package, file } => match package {
                Some(package) => write!(
                    f,
                    "`{}` is a build input of `{package}`; a full rebuild picks it up",
                    file.display()
                ),
                None => write!(
                    f,
                    "`{}` is a build input; a full rebuild picks it up",
                    file.display()
                ),
            },
            Self::LayoutChanged { records } => {
                write!(f, "{}; restarting to keep memory safe", records.join("; "))
            }
            Self::StateTypeChanged { changes } => write!(
                f,
                "{}; the patch could not reach the component",
                changes.join("; ")
            ),
            Self::HotPatchUnavailable { reason } => {
                write!(f, "hot patching is unavailable for this app: {reason}")
            }
            Self::EndpointNotLoopback { endpoint } => write!(
                f,
                "the devtools endpoint {endpoint} is not loopback; patches go to loopback only"
            ),
            Self::PatchBudget {
                patches,
                bytes,
                budget,
            } => write!(
                f,
                "the patch budget is spent ({patches} patches / {bytes} bytes loaded would pass \
                 {} patches or {} bytes)",
                budget.patches, budget.bytes
            ),
            Self::PatchOutcomeUnknown { detail } => write!(
                f,
                "the app's answer to the patch was lost ({detail}); the patch may be live"
            ),
            Self::PatchRefused { detail } => write!(f, "the app refused the patch: {detail}"),
            Self::DevtoolsFailed { detail } => {
                write!(f, "the devtools connection failed: {detail}")
            }
            Self::StaleSeamCall { image, symbol } => {
                let image = if *image == 0 {
                    "the base image".to_string()
                } else {
                    format!("patch {image}")
                };
                write!(
                    f,
                    "stale code in {image} still calls the replaced seam `{symbol}`"
                )
            }
            Self::UnresolvedMissedKey {
                image,
                link_address,
            } => write!(
                f,
                "a missed seam key (image {image}, {link_address:#x}) maps to no known symbol"
            ),
            Self::NoSeamHit => write!(f, "the patch reached no component (no seam hit)"),
            Self::BuilderUnsupported { detail } => {
                write!(f, "hot-patch builder unsupported: {detail}")
            }
        }
    }
}

/// What one [`HotSession::on_change`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The patch is live and reached `components` component rebuilds.
    Patched { ms: u64, components: u64 },
    /// Nothing the running image was built from changed.
    NoChange,
    /// A replayed crate did not compile; the running app is untouched.
    CompileFailed { diagnostics: Vec<String> },
    /// The app must be relaunched (a fresh fat session).
    RestartRequired(RestartReason),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Patched { ms, components } => {
                write!(f, "patched in {ms} ms ({components} components rebuilt)")
            }
            Self::NoChange => write!(f, "no change to the running app"),
            Self::CompileFailed { .. } => {
                write!(f, "compile failed; the running app is untouched")
            }
            Self::RestartRequired(reason) => write!(f, "restart required: {reason}"),
        }
    }
}

/// Why [`start_desktop`] produced no session.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// This build cannot be hot-patched; run it cold instead.
    #[error("restart required: {0}")]
    RestartRequired(RestartReason),
    /// The fat build did not compile.
    #[error("the hot-patch fat build failed")]
    FatBuildFailed { diagnostics: Vec<String> },
    /// The fat image could not be launched, or exited before it announced
    /// its devtools endpoint.
    #[error("launching the fat image failed: {detail}")]
    Launch { detail: String },
}

impl From<HotpatchError> for StartError {
    fn from(err: HotpatchError) -> Self {
        Self::RestartRequired(RestartReason::builder(&err))
    }
}

/// Checks that the dev profile builds with full debug info, the DWARF type
/// information the layout gate reads. The value is resolved the way cargo
/// resolves it: [`DEV_DEBUG_ENV`] first (CI sets it to `0`), then a
/// `[profile.dev] debug` in the `.cargo/config.toml` (or legacy `config`)
/// of `project_root` and each of its ancestors, nearest first, then in
/// `$CARGO_HOME`'s, then the workspace root's `Cargo.toml`; unset means
/// cargo's default, `true`. Anything below `2`/`true`/`"full"` (`0`, `1`,
/// `false`, `"limited"`, `"line-tables-only"`, ...) is
/// [`HotpatchError::BuilderUnsupported`] whose detail starts
/// `debuginfo off`.
pub fn check_debuginfo(
    env: &dyn EnvLookup,
    project_root: &Path,
    workspace_root: &Path,
) -> Result<(), HotpatchError> {
    let (value, source) = match env.get(DEV_DEBUG_ENV) {
        Some(value) => (value, format!("`{DEV_DEBUG_ENV}`")),
        None => match config_debug(env, project_root)? {
            Some(found) => found,
            None => {
                let manifest = workspace_root.join("Cargo.toml");
                match profile_debug(&manifest)? {
                    Some(value) => (value, format!("`{}`", manifest.display())),
                    None => return Ok(()),
                }
            }
        },
    };
    if matches!(value.trim(), "2" | "true" | "full") {
        Ok(())
    } else {
        Err(HotpatchError::unsupported(format!(
            "debuginfo off: {source} sets the dev profile's `debug = {value}`; hot patching reads \
             DWARF type information and needs `debug = true`"
        )))
    }
}

/// The first `[profile.dev] debug` among the cargo config files that apply
/// to a build in `project_root`, with the file that set it.
fn config_debug(
    env: &dyn EnvLookup,
    project_root: &Path,
) -> Result<Option<(String, String)>, HotpatchError> {
    let mut dirs: Vec<PathBuf> = project_root
        .ancestors()
        .map(|dir| dir.join(".cargo"))
        .collect();
    let cargo_home = env
        .get("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env.get("HOME").map(|home| Path::new(&home).join(".cargo")));
    dirs.extend(cargo_home);
    for dir in dirs {
        for name in ["config.toml", "config"] {
            let file = dir.join(name);
            if let Some(value) = profile_debug(&file)? {
                return Ok(Some((value, format!("`{}`", file.display()))));
            }
        }
    }
    Ok(None)
}

/// `[profile.dev] debug` in the TOML file at `path`, rendered as cargo
/// spells it on the command line; `None` when the file or key is absent.
fn profile_debug(path: &Path) -> Result<Option<String>, HotpatchError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(HotpatchError::io(
                format!("reading `{}`", path.display()),
                err,
            ));
        }
    };
    let doc: toml::Table = toml::from_str(&text).map_err(|err| {
        HotpatchError::unsupported(format!("`{}` is not valid TOML: {err}", path.display()))
    })?;
    let debug = doc
        .get("profile")
        .and_then(|p| p.get("dev"))
        .and_then(|d| d.get("debug"));
    Ok(debug.map(|value| match value {
        toml::Value::String(text) => text.clone(),
        other => other.to_string(),
    }))
}

/// The resolved `cargo rustc` invocation of a fat build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FatBuild {
    pub program: String,
    pub args: Vec<String>,
    /// Added to the child's environment: the workspace wrapper, the
    /// capture scope and the no-link action.
    pub env: Vec<(String, String)>,
}

/// `cargo rustc -p <package> --bin <bin> --message-format
/// json-diagnostic-rendered-ansi --features ... -- -Csave-temps=true
/// -Clink-dead-code -Clinker=<frust>` with `defines` (the desktop plan's
/// compile-time environment), then `RUSTC_WORKSPACE_WRAPPER` and the
/// capture variable naming `frust_exe` and `scope_dir`, then `link`'s
/// variables. The `--` flags reach only the tip bin.
pub fn fat_build_command(
    package: &str,
    bin: &str,
    features: &[&str],
    defines: &[(String, String)],
    frust_exe: &Path,
    scope_dir: &Path,
    link: &LinkAction,
) -> FatBuild {
    let mut args: Vec<String> = [
        "rustc",
        "-p",
        package,
        "--bin",
        bin,
        "--message-format",
        "json-diagnostic-rendered-ansi",
    ]
    .map(str::to_string)
    .to_vec();
    for feature in features {
        args.push("--features".to_string());
        args.push((*feature).to_string());
    }
    args.push("--".to_string());
    args.push("-Csave-temps=true".to_string());
    args.push("-Clink-dead-code".to_string());
    args.push(linker_arg(frust_exe));
    let mut env = defines.to_vec();
    env.extend(capture::wrapper_env(frust_exe, scope_dir));
    env.extend(link.env_vars());
    FatBuild {
        program: "cargo".to_string(),
        args,
        env,
    }
}

/// The session's accepted-layout and accepted-seam sets and the directory
/// they are recorded in.
#[derive(Debug)]
pub struct AcceptedSets {
    dir: PathBuf,
    layouts: LayoutTable,
    seams: SeamSet,
}

impl AcceptedSets {
    /// Starts a session's sets: deletes and recreates
    /// `<target>/frust-hotpatch/<session>` (removing a previous session's
    /// patches), writes `layout-base.json` there and seeds both sets from
    /// the base.
    pub fn begin(
        target_dir: &Path,
        session: &str,
        base_layouts: LayoutTable,
        base_seams: SeamSet,
    ) -> Result<Self, HotpatchError> {
        let dir = thin_link::reset_session_dir(target_dir, session)?;
        base_layouts.write_base(&dir)?;
        Ok(Self {
            dir,
            layouts: base_layouts,
            seams: base_seams,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn layouts(&self) -> &LayoutTable {
        &self.layouts
    }

    pub fn seams(&self) -> &SeamSet {
        &self.seams
    }

    /// The host gates: every type the candidate shares with the accepted
    /// set keeps its layout, and every component keeps its `State`
    /// identity. Passing returns the candidate's seam instances.
    pub fn check(&self, layouts: &LayoutTable, seams: SeamSet) -> Result<SeamSet, RestartReason> {
        let changed = layout::diff(layouts, &self.layouts);
        if !changed.is_empty() {
            return Err(RestartReason::LayoutChanged {
                records: changed.iter().map(ToString::to_string).collect(),
            });
        }
        let report = seams::check(seams, &self.seams);
        if !report.changed.is_empty() {
            return Err(RestartReason::StateTypeChanged {
                changes: report.changed.iter().map(ToString::to_string).collect(),
            });
        }
        Ok(report.present)
    }

    /// Merges an applied patch's entries (never removing one) and rewrites
    /// [`LAYOUTS_ACCEPTED_FILE`].
    pub fn accept(&mut self, layouts: &LayoutTable, seams: &SeamSet) -> Result<(), HotpatchError> {
        layout::merge(&mut self.layouts, layouts);
        seams::merge(&mut self.seams, seams);
        self.layouts.write(&self.dir.join(LAYOUTS_ACCEPTED_FILE))
    }
}

/// What a recompile produced.
enum Compiled {
    /// No unit of the running image changed.
    Nothing,
    /// A replayed crate failed to compile.
    Failed { diagnostics: Vec<String> },
    /// The candidate's layout table (of the freshly compiled objects) and
    /// seam instances (of everything the patch will carry).
    Candidate {
        layouts: LayoutTable,
        seams: SeamSet,
    },
}

/// A linked patch, ready to send.
struct LinkedPatch {
    bytes: Vec<u8>,
    table: JumpTableWire,
    symbols: ImageSymbols,
}

/// The build half of a session: classification, replay, thin link. A
/// trait so the session's decisions are testable without a toolchain.
trait PatchBuilder: Send {
    fn classify(&self, path: &Path) -> PathClass;
    fn compile(&mut self, units: &BTreeSet<ReplayUnit>) -> Result<Compiled, HotpatchError>;
    /// Links patch number `n` against the process whose anchor is at
    /// `anchor_runtime`.
    fn link(&mut self, n: u32, anchor_runtime: u64) -> Result<LinkedPatch, HotpatchError>;
}

/// How the session reaches the app.
enum AppLink {
    /// Hot patching is off for this process; every change restarts.
    RestartOnly { reason: String },
    Live {
        client: DevtoolsClient,
        endpoint: SocketAddr,
        pid: u32,
        anchor_runtime: u64,
    },
}

/// Connects to the app's devtools endpoint and decides whether this
/// session can patch it. Nothing is sent to a non-loopback endpoint (an
/// IPv4-mapped IPv6 address counts as non-loopback); an app without
/// `HotPatch` gives the precondition its `hotpatch_info` refusal names.
fn attach_app(endpoint: SocketAddr, token: Option<&str>, triple: &str) -> AppLink {
    let restart_only = |reason: String| AppLink::RestartOnly { reason };
    if !endpoint.ip().is_loopback() {
        return restart_only(RestartReason::EndpointNotLoopback { endpoint }.to_string());
    }
    let client = match DevtoolsClient::connect(endpoint, DEVTOOLS_TIMEOUT, token) {
        Ok(client) => client,
        Err(err) => return restart_only(format!("{err:#}")),
    };
    let handshake = match client.handshake() {
        Ok(handshake) => handshake,
        Err(err) => return restart_only(format!("{err:#}")),
    };
    if !handshake.capabilities.contains(&Capability::HotPatch) {
        let reason = match client.hotpatch_info() {
            Err(err) if is_not_supported(&err) => err
                .downcast_ref::<DevtoolsRpcError>()
                .map(|rpc| rpc.message.clone())
                .unwrap_or_else(|| format!("{err:#}")),
            _ => "the app does not advertise the HotPatch capability (is it built with \
                  `frust/hotpatch`?)"
                .to_string(),
        };
        return restart_only(reason);
    }
    let info = match client.hotpatch_info() {
        Ok(info) => info,
        Err(err) => return restart_only(format!("{err:#}")),
    };
    if info.anchor_runtime == 0 {
        return restart_only("the app has registered no hot-patch anchor".to_string());
    }
    if info.triple != triple {
        return restart_only(format!(
            "the app runs `{}`, the build targets `{triple}`",
            info.triple
        ));
    }
    AppLink::Live {
        client,
        endpoint,
        pid: info.pid,
        anchor_runtime: info.anchor_runtime,
    }
}

/// One running app process and everything needed to patch it.
pub struct HotSession {
    builder: Box<dyn PatchBuilder>,
    app: AppLink,
    accepted: AcceptedSets,
    /// The symbol tables of every image loaded in the process: the base at
    /// index 0, then each applied patch in load order.
    images: Vec<ImageSymbols>,
    budget: Budget,
    /// Set by the first restart; every later change answers it.
    restart: Option<RestartReason>,
    next_patch_id: u64,
}

impl HotSession {
    /// Why this session can only restart, when it can.
    pub fn restart_only_reason(&self) -> Option<&str> {
        match &self.app {
            AppLink::RestartOnly { reason } => Some(reason),
            AppLink::Live { .. } => None,
        }
    }

    /// The app process's pid, as it reported it.
    pub fn pid(&self) -> Option<u32> {
        match &self.app {
            AppLink::Live { pid, .. } => Some(*pid),
            AppLink::RestartOnly { .. } => None,
        }
    }

    /// `<target>/frust-hotpatch/<session>`: patches, `layout-base.json`,
    /// `layouts-accepted.json`.
    pub fn session_dir(&self) -> &Path {
        self.accepted.dir()
    }

    /// The accepted sets, as merged so far.
    pub fn accepted(&self) -> &AcceptedSets {
        &self.accepted
    }

    /// Patches the running app with `paths`' changes, or says why it must
    /// be relaunched. `paths` are the changed files as the watcher reports
    /// them.
    pub fn on_change(&mut self, paths: &[PathBuf]) -> Outcome {
        if let Some(reason) = &self.restart {
            return Outcome::RestartRequired(reason.clone());
        }
        let outcome = self.change(paths);
        if let Outcome::RestartRequired(reason) = &outcome {
            self.restart = Some(reason.clone());
        }
        outcome
    }

    fn change(&mut self, paths: &[PathBuf]) -> Outcome {
        let started = Instant::now();
        let restart = Outcome::RestartRequired;
        let (client, pid, anchor_runtime) = match &self.app {
            AppLink::RestartOnly { reason } => {
                return restart(RestartReason::HotPatchUnavailable {
                    reason: reason.clone(),
                });
            }
            AppLink::Live {
                client,
                endpoint,
                pid,
                anchor_runtime,
            } => {
                let peer_loopback = client.peer_addr().is_some_and(|a| a.ip().is_loopback());
                if !endpoint.ip().is_loopback() || !peer_loopback {
                    return restart(RestartReason::EndpointNotLoopback {
                        endpoint: *endpoint,
                    });
                }
                (client, *pid, *anchor_runtime)
            }
        };

        let units = match classify_paths(self.builder.as_ref(), paths) {
            Ok(units) => units,
            Err(reason) => return restart(reason),
        };
        if units.is_empty() {
            return Outcome::NoChange;
        }
        let (layouts, seams) = match self.builder.compile(&units) {
            Ok(Compiled::Nothing) => return Outcome::NoChange,
            Ok(Compiled::Failed { diagnostics }) => return Outcome::CompileFailed { diagnostics },
            Ok(Compiled::Candidate { layouts, seams }) => (layouts, seams),
            Err(err) => return restart(RestartReason::builder(&err)),
        };
        let present = match self.accepted.check(&layouts, seams) {
            Ok(present) => present,
            Err(reason) => return restart(reason),
        };

        let info = match client.hotpatch_info() {
            Ok(info) => info,
            Err(err) => {
                return restart(RestartReason::DevtoolsFailed {
                    detail: format!("{err:#}"),
                });
            }
        };
        if !info.pending_layout_mismatches.is_empty() {
            return restart(RestartReason::LayoutChanged {
                records: info.pending_layout_mismatches,
            });
        }
        if info.pid != pid || info.anchor_runtime != anchor_runtime {
            return restart(RestartReason::BuilderUnsupported {
                detail: format!(
                    "the app now reports pid {} / anchor {:#x}, the session started against \
                     pid {pid} / anchor {anchor_runtime:#x}",
                    info.pid, info.anchor_runtime
                ),
            });
        }
        let applied_before = info.patches_applied;
        if applied_before as usize + 1 != self.images.len() {
            return restart(RestartReason::BuilderUnsupported {
                detail: format!(
                    "the app reports {applied_before} loaded patches, this session applied {}",
                    self.images.len() - 1
                ),
            });
        }
        let n = applied_before.saturating_add(1);
        if let Some(reason) = self.budget.exceeded(n, info.patch_bytes_loaded) {
            return restart(reason);
        }

        let linked = match self.builder.link(n, anchor_runtime) {
            Ok(linked) => linked,
            Err(err) => return restart(RestartReason::builder(&err)),
        };
        let len = linked.bytes.len() as u64;
        if let Some(reason) = self
            .budget
            .exceeded(n, info.patch_bytes_loaded.saturating_add(len))
        {
            return restart(reason);
        }

        let patch_id = self.next_patch_id;
        self.next_patch_id += 1;
        if let Err(err) = client.upload_patch(patch_id, &linked.bytes) {
            return restart(RestartReason::DevtoolsFailed {
                detail: format!("{err:#}"),
            });
        }
        let params = ApplyPatchParams {
            patch_id,
            len,
            pid,
            anchor_runtime,
            table: linked.table,
            expected_seams: u32::try_from(present.len()).unwrap_or(u32::MAX),
        };
        let outcome = match client.apply_patch(&params) {
            Ok(outcome) => outcome,
            Err(err) => {
                let detail = format!("{err:#}");
                return restart(if err.downcast_ref::<DevtoolsRpcError>().is_some() {
                    RestartReason::PatchRefused { detail }
                } else {
                    RestartReason::PatchOutcomeUnknown { detail }
                });
            }
        };
        self.settle(outcome, &layouts, &present, linked.symbols, started)
    }

    /// Maps an `apply_patch` answer to the session's outcome, merging the
    /// candidate into the accepted sets when the patch is live and no
    /// layout mismatch was recorded.
    fn settle(
        &mut self,
        outcome: PatchOutcome,
        layouts: &LayoutTable,
        present: &SeamSet,
        symbols: ImageSymbols,
        started: Instant,
    ) -> Outcome {
        let restart = Outcome::RestartRequired;
        if !outcome.layout_mismatches.is_empty() {
            return restart(RestartReason::LayoutChanged {
                records: outcome.layout_mismatches,
            });
        }
        if !outcome.applied {
            return restart(RestartReason::PatchRefused {
                detail: "the app answered `applied: false` without layout records".to_string(),
            });
        }
        if let Err(err) = self.accepted.accept(layouts, present) {
            return restart(RestartReason::builder(&err));
        }
        self.images.push(symbols);
        let newest = outcome.patches_applied;
        if newest as usize + 1 != self.images.len() {
            return restart(RestartReason::BuilderUnsupported {
                detail: format!(
                    "the app reports {newest} loaded patches after this one, this session \
                     applied {}",
                    self.images.len() - 1
                ),
            });
        }
        if outcome.seam_hits == 0 {
            return restart(RestartReason::NoSeamHit);
        }
        for key in &outcome.seam_fall_throughs {
            if let Some(reason) = self.stale_call(*key, newest, present) {
                return restart(reason);
            }
        }
        Outcome::Patched {
            ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            components: outcome.seam_hits,
        }
    }

    /// Classifies one missed seam key: `None` when it is harmless (a caller
    /// in the newest patch, or a stale caller of a seam the patch did not
    /// replace).
    fn stale_call(&self, key: MissedKey, newest: u32, present: &SeamSet) -> Option<RestartReason> {
        if key.image == newest {
            return None;
        }
        let unresolved = RestartReason::UnresolvedMissedKey {
            image: key.image,
            link_address: key.link_address,
        };
        let Some(image) = self.images.get(key.image as usize) else {
            return Some(unresolved);
        };
        let Some(symbol) = image.symbol_at(key.link_address) else {
            return Some(unresolved);
        };
        present
            .contains_symbol(symbol)
            .then(|| RestartReason::StaleSeamCall {
                image: key.image,
                symbol: symbol.to_string(),
            })
    }
}

/// The replayable units `paths` touch, or the restart a path forces. A path
/// the graph does not know as written is retried in its canonical form (the
/// watcher and `cargo metadata` can spell one directory differently).
fn classify_paths(
    builder: &dyn PatchBuilder,
    paths: &[PathBuf],
) -> Result<BTreeSet<ReplayUnit>, RestartReason> {
    let mut units = BTreeSet::new();
    for path in paths {
        let mut class = builder.classify(path);
        if class == PathClass::Unaffected
            && let Ok(canonical) = std::fs::canonicalize(path)
            && canonical != *path
        {
            class = builder.classify(&canonical);
        }
        match class {
            PathClass::Replayable { units: found } => units.extend(found),
            PathClass::LocalNonMember { package } => {
                return Err(RestartReason::PathDependencyChanged {
                    package,
                    file: path.clone(),
                });
            }
            PathClass::BuildInput { package } => {
                return Err(RestartReason::BuildInputChanged {
                    package,
                    file: path.clone(),
                });
            }
            PathClass::Unaffected => {}
        }
    }
    Ok(units)
}

/// The process and environment seams a session runs through.
pub struct SessionHost<'a> {
    pub runner: Arc<dyn ProcessRunner + Send + Sync>,
    pub env: &'a dyn EnvLookup,
    /// The `frust` executable named as wrapper and linker.
    pub frust_exe: PathBuf,
}

impl SessionHost<'static> {
    /// `runner` with this process's environment and executable.
    pub fn current(runner: Arc<dyn ProcessRunner + Send + Sync>) -> Result<Self, HotpatchError> {
        Ok(Self {
            runner,
            env: &RealEnv,
            frust_exe: capture::frust_exe()?,
        })
    }
}

/// What to build and run.
#[derive(Debug, Clone, Copy)]
pub struct DesktopStart<'a> {
    /// The project root: cargo's working directory and the app's.
    pub root: &'a Path,
    pub info: &'a BuildInfo,
    /// The tip package.
    pub package: &'a str,
    /// The bin to run; may be omitted when the package has exactly one.
    pub bin: Option<&'a str>,
}

/// Builds the app fat, launches the image and attaches a session to it.
/// On success the caller owns the child's [`StreamHandle`] (its output
/// after the discovery line, and its lifetime); relaunching means killing
/// it and starting again, which resets the session. `on_line` receives the
/// build's rendered diagnostics and the app's output up to its discovery
/// line (token redacted).
pub fn start_desktop(
    host: &SessionHost<'_>,
    start: &DesktopStart<'_>,
    on_line: &mut dyn FnMut(&str),
) -> Result<(HotSession, StreamHandle), StartError> {
    let runner: &dyn ProcessRunner = &*host.runner;
    let unsupported =
        |detail: String| StartError::RestartRequired(RestartReason::BuilderUnsupported { detail });
    if start.info.mode != BuildMode::Debug {
        return Err(unsupported(format!(
            "hot patching needs a debug build, not {:?}",
            start.info.mode
        )));
    }
    let manifest =
        manifest::load_optional(start.root).map_err(|err| unsupported(format!("{err:#}")))?;
    let budget = Budget::from_section(manifest.as_ref().and_then(|m| m.hotpatch.as_ref()));

    let metadata = super::graph::cargo_metadata(runner, &start.root.join("Cargo.toml"), None)?;
    let mut graph = WorkspaceGraph::from_metadata(&metadata, start.package, start.bin)?;
    check_debuginfo(host.env, start.root, graph.workspace_root())?;
    let target_dir = target_directory(&metadata)?;

    let rustc_version = capture::rustc_version(runner)?;
    let triple = host_triple(&rustc_version)?;
    let target = Target::from_triple(&triple)?;
    let flavor = LinkerFlavor::for_triple(&triple)?;
    let features = start.info.mode.session_cargo_features(true);
    let scope = ScopeInputs {
        tip: start.package.to_string(),
        triple: triple.clone(),
        profile: "dev".to_string(),
        features: features.iter().map(|f| (*f).to_string()).collect(),
        rustflags: rustflags(host.env),
        rustc_version,
    };
    let scope_dir = prepare_scope_dir(&target_dir, &scope)?;
    let members: Vec<String> = graph
        .packages()
        .iter()
        .filter(|p| p.member && p.name != start.package)
        .map(|p| p.name.clone())
        .collect();
    capture::bust_fingerprints(
        &capture::fingerprint_dir(&target_dir, None, "dev"),
        &scope_dir,
        start.package,
        &members,
    )?;

    let fat_dir = hotpatch_root(&target_dir)
        .join(FAT_DIR)
        .join(scope.dir_name()?);
    std::fs::create_dir_all(&fat_dir)
        .map_err(|err| HotpatchError::io(format!("creating `{}`", fat_dir.display()), err))?;
    let link = LinkAction {
        mode: LinkMode::NoLink,
        args_file: fat_dir.join("link-args.json"),
        err_file: Some(fat_dir.join("link-err.txt")),
    };
    remove_stale(&link.args_file)?;
    let tip_bin = graph.tip_bin();
    let defines = desktop_run::desktop_plan(start.root, start.info).env;
    let fat = fat_build_command(
        start.package,
        &tip_bin.target,
        features,
        &defines,
        &host.frust_exe,
        &scope_dir,
        &link,
    );
    run_fat_build(runner, &fat, start.root, on_line)?;

    let link_args = read_link_args(&link.args_file)?;
    let records = load_records(&scope_dir)?;
    let tip_record = records.get(&tip_bin.record_key()).ok_or_else(|| {
        HotpatchError::unsupported(format!(
            "the fat build captured no `{}` invocation",
            tip_bin.record_key()
        ))
    })?;
    let tip_env = replay_env(tip_record);
    let linker = fat_link::linker_program(custom_linker(host.env, &triple).as_deref())?;
    let exe = fat_dir.join(&tip_bin.target);
    let fat_out = fat_link::fat_link(
        runner,
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

    let rlibs = member_rlibs(&link_args, &graph);
    let tip_objects = tip_objects(&link_args);
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
    seed_dep_info(&mut graph, &records);

    let plan = desktop_run::desktop_exe_plan(start.root, &fat_out.exe, start.info);
    let mut child =
        desktop_run::spawn_desktop_plan(runner, &plan).map_err(|err| StartError::Launch {
            detail: format!("{err:#}"),
        })?;
    let app = match read_discovery(&mut child, on_line) {
        Announced::Endpoint(discovery) => attach_app(
            SocketAddr::from((Ipv4Addr::LOCALHOST, discovery.port)),
            discovery.token.as_deref(),
            &triple,
        ),
        Announced::Failure(reason) => AppLink::RestartOnly { reason },
        Announced::Exited => {
            child.kill();
            return Err(StartError::Launch {
                detail: "the app exited before announcing its devtools endpoint".to_string(),
            });
        }
    };

    let images = vec![cache.symbols().clone()];
    let builder = DesktopBuilder {
        runner: Arc::clone(&host.runner),
        graph,
        records,
        modified: super::graph::ModifiedSet::new(),
        dirty: BTreeSet::new(),
        rlibs,
        tip_link_args: link_args,
        tip_env,
        tip_bin,
        cache,
        target,
        flavor,
        linker,
        target_dir,
        session,
        session_dir: accepted.dir().to_path_buf(),
        scope_dir,
        frust_exe: host.frust_exe.clone(),
        crates,
        tip_replays: 0,
    };
    Ok((
        HotSession {
            builder: Box::new(builder),
            app,
            accepted,
            images,
            budget,
            restart: None,
            next_patch_id: 1,
        },
        child,
    ))
}

/// What the fat image's output announced.
enum Announced {
    Endpoint(Discovery),
    /// The devtools service did not start, or said nothing in time.
    Failure(String),
    Exited,
}

/// Reads the child's output up to its devtools discovery line, forwarding
/// every line before it (token redacted) to `on_line`.
fn read_discovery(child: &mut StreamHandle, on_line: &mut dyn FnMut(&str)) -> Announced {
    let deadline = Instant::now() + DISCOVERY_DEADLINE;
    loop {
        match child.lines.try_recv() {
            Ok(line) => {
                on_line(&redact_discovery_token(&line));
                if let Some(discovery) = parse_discovery_line(&line) {
                    return Announced::Endpoint(discovery);
                }
                if let Some(reason) = parse_failure_line(&line) {
                    return Announced::Failure(format!(
                        "the devtools service did not start: {reason}"
                    ));
                }
            }
            Err(TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(DISCOVERY_POLL);
            }
            Err(TryRecvError::Empty) => {
                return Announced::Failure(format!(
                    "the app announced no devtools endpoint within {}s",
                    DISCOVERY_DEADLINE.as_secs()
                ));
            }
            Err(TryRecvError::Disconnected) => return Announced::Exited,
        }
    }
}

/// Runs the fat build, forwarding cargo's rendered diagnostics and any
/// non-JSON line to `on_line`. A failed build is
/// [`StartError::FatBuildFailed`] with its error diagnostics.
fn run_fat_build(
    runner: &dyn ProcessRunner,
    fat: &FatBuild,
    root: &Path,
    on_line: &mut dyn FnMut(&str),
) -> Result<(), StartError> {
    let args: Vec<&str> = fat.args.iter().map(String::as_str).collect();
    let env: Vec<(&str, &str)> = fat
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut errors = Vec::new();
    let output = runner
        .run_streaming(
            &fat.program,
            &args,
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
                    on_line(rendered.trim_end());
                    if inner.and_then(|m| m.get("level")).and_then(|l| l.as_str()) == Some("error")
                    {
                        errors.push(rendered.to_string());
                    }
                }
                Err(_) => on_line(line),
            },
        )
        .map_err(|err| HotpatchError::Process {
            detail: format!("failed to spawn the fat build: {err:#}"),
        })?;
    if output.success {
        return Ok(());
    }
    let stderr = output.stderr.trim();
    if !stderr.is_empty() {
        errors.push(stderr.to_string());
    }
    Err(StartError::FatBuildFailed {
        diagnostics: errors,
    })
}

/// `cargo metadata`'s `target_directory`.
fn target_directory(metadata: &str) -> Result<PathBuf, HotpatchError> {
    let value: serde_json::Value = serde_json::from_str(metadata).map_err(|err| {
        HotpatchError::unsupported(format!("unreadable `cargo metadata` output: {err}"))
    })?;
    value
        .get("target_directory")
        .and_then(|dir| dir.as_str())
        .map(PathBuf::from)
        .ok_or_else(|| HotpatchError::unsupported("`cargo metadata` names no target_directory"))
}

/// The `host:` line of `rustc -vV`: the triple a plain `cargo rustc` builds.
fn host_triple(rustc_version: &str) -> Result<String, HotpatchError> {
    rustc_version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(|triple| triple.trim().to_string())
        .ok_or_else(|| HotpatchError::unsupported("`rustc -vV` reports no `host:` triple"))
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
fn remove_stale(path: &Path) -> Result<(), HotpatchError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(HotpatchError::io(
            format!("removing `{}`", path.display()),
            err,
        )),
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

/// The layout gate's refusal of an input whose DWARF holds no type at all.
const NO_TYPE_INFORMATION: &str = "without type information";

/// The layout gate's refusal of an input with no DWARF at all.
const NO_DWARF: &str = "carries no DWARF";

/// The tip objects whose DWARF describes at least one type. A bin that
/// declares and uses no type of its own (a one-line `main`) compiles to
/// objects with no type, or no DWARF at all (an allocator shim), which
/// [`layout::extract`] refuses as if debug info were reduced; such an
/// object has no type to compare, so it is left out of the layout gate.
/// At least one object must still carry DWARF, the dev profile's debug
/// level is checked separately ([`check_debuginfo`]), and every rlib input
/// is held to the stricter rule.
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

/// Every member target's crate name: the crates whose types the layout
/// gate fingerprints.
fn replayable_crates(graph: &WorkspaceGraph) -> Vec<String> {
    let names: BTreeSet<String> = graph
        .units()
        .iter()
        .map(|unit| unit.record_key().crate_name)
        .collect();
    names.into_iter().collect()
}

/// Seeds the graph's dep-info from each captured compile's
/// `<out-dir>/<crate><extra-filename>.d`, where the fat build left it, so a
/// non-`.rs` input or a bin-only module is classified from the start. A
/// record whose dep-info cannot be found is skipped: the graph then falls
/// back to its conservative rules.
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
        if let Ok(files) = super::graph::read_dep_info(&file, &cwd) {
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

/// The real build half: replay through captured invocations, thin link
/// against the fat image.
struct DesktopBuilder {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    graph: WorkspaceGraph,
    records: BTreeMap<RecordKey, RustcRecord>,
    modified: super::graph::ModifiedSet,
    /// Units recorded as changed whose replay has not yet succeeded; they
    /// are replayed again with the next change, so a patch never links a
    /// stale object after a failed compile.
    dirty: BTreeSet<ReplayUnit>,
    /// The current rlib of each lib unit in the image.
    rlibs: BTreeMap<ReplayUnit, PathBuf>,
    /// The tip's latest captured link: the fat build's until the tip bin is
    /// replayed.
    tip_link_args: Vec<String>,
    tip_env: Vec<(String, String)>,
    tip_bin: ReplayUnit,
    cache: SymbolCache,
    target: Target,
    flavor: LinkerFlavor,
    linker: String,
    target_dir: PathBuf,
    session: String,
    session_dir: PathBuf,
    scope_dir: PathBuf,
    frust_exe: PathBuf,
    crates: Vec<String>,
    tip_replays: u32,
}

impl DesktopBuilder {
    /// Every modified lib's rlib, dependents before their dependencies (the
    /// order a static link resolves archives in).
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

    /// Replays the tip bin with its link step intercepted, so the patch
    /// links its fresh objects. `Ok(Err(diagnostics))` for a compile error.
    fn replay_tip_bin(&mut self) -> Result<Result<Vec<PathBuf>, Vec<String>>, HotpatchError> {
        let key = self.tip_bin.record_key();
        let record = self.records.get(&key).ok_or_else(|| {
            HotpatchError::unsupported(format!("no captured rustc invocation `{key}`"))
        })?;
        let rustc = record
            .rustc()
            .ok_or_else(|| HotpatchError::unsupported(format!("the `{key}` capture has no rustc")))?
            .to_string();
        let mut args = replay_args(record)?;
        args.push(linker_arg(&self.frust_exe));
        self.tip_replays += 1;
        let link = LinkAction {
            mode: LinkMode::NoLink,
            args_file: self
                .session_dir
                .join(format!("tip-link-args-{}.json", self.tip_replays)),
            err_file: Some(self.session_dir.join("tip-link-err.txt")),
        };
        let mut env = replay_env(record);
        env.push((
            CAPTURE_ENV.to_string(),
            self.scope_dir.to_string_lossy().into_owned(),
        ));
        env.extend(link.env_vars());
        let removed: Vec<&str> = STRIPPED_ENV
            .iter()
            .copied()
            .filter(|name| !env.iter().any(|(set, _)| set == name))
            .collect();
        let cwd = self.graph.replay_cwd(&self.tip_bin);
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let env_refs: Vec<(&str, &str)> =
            env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let output = self
            .runner
            .run_streaming_scrubbed(&rustc, &argv, Some(&cwd), &env_refs, &removed, &mut |_| {})
            .map_err(|err| HotpatchError::Process {
                detail: format!(
                    "failed to spawn `{rustc}` to replay {}: {err:#}",
                    self.tip_bin
                ),
            })?;
        let (_, mut diagnostics) = parse_notifications(&output.stderr, &cwd)?;
        if !output.success {
            if let Ok(report) = std::fs::read_to_string(link.err_file.as_ref().expect("set above"))
            {
                diagnostics.push(report);
            }
            return Ok(Err(diagnostics));
        }
        self.tip_link_args = read_link_args(&link.args_file)?;
        Ok(Ok(tip_objects(&self.tip_link_args)))
    }
}

impl PatchBuilder for DesktopBuilder {
    fn classify(&self, path: &Path) -> PathClass {
        self.graph.classify(path)
    }

    fn compile(&mut self, units: &BTreeSet<ReplayUnit>) -> Result<Compiled, HotpatchError> {
        let plan = self.modified.record_change(&self.graph, units)?;
        let mut pending: BTreeSet<ReplayUnit> = plan.replay.into_iter().collect();
        pending.extend(self.dirty.iter().cloned());
        if pending.is_empty() {
            return Ok(Compiled::Nothing);
        }
        self.dirty = pending.clone();
        let order = self.graph.replay_order(&pending)?;
        let (bins, libs): (Vec<ReplayUnit>, Vec<ReplayUnit>) =
            order.into_iter().partition(|unit| *unit == self.tip_bin);

        let runner: &dyn ProcessRunner = &*self.runner;
        let outcomes = replay_units(runner, &self.graph, &self.records, &libs)?;
        let mut fresh = Vec::new();
        for outcome in outcomes {
            if !outcome.success {
                return Ok(Compiled::Failed {
                    diagnostics: outcome.diagnostics,
                });
            }
            let rlib = outcome.rlib()?.to_path_buf();
            if let Some(dep_info) = outcome.dep_info() {
                let cwd = self.graph.replay_cwd(&outcome.unit);
                if let Ok(files) = super::graph::read_dep_info(dep_info, &cwd) {
                    self.graph.set_dep_info(outcome.unit.clone(), files);
                }
            }
            self.rlibs.insert(outcome.unit.clone(), rlib.clone());
            self.dirty.remove(&outcome.unit);
            fresh.push(rlib);
        }
        if !bins.is_empty() {
            match self.replay_tip_bin()? {
                Ok(objects) => fresh.extend(typed_objects(&objects, &self.crates)?),
                Err(diagnostics) => return Ok(Compiled::Failed { diagnostics }),
            }
            self.dirty.remove(&self.tip_bin);
        }

        // Only the freshly compiled objects can differ from what the
        // accepted set already holds: every other input of the patch was
        // checked, and accepted, with an earlier one.
        let layouts = if fresh.is_empty() {
            LayoutTable::default()
        } else {
            layout::extract(&fresh, &self.crates)?.table
        };
        let seams = SeamSet::from_inputs(&self.patch_inputs()?)?;
        Ok(Compiled::Candidate { layouts, seams })
    }

    fn link(&mut self, n: u32, anchor_runtime: u64) -> Result<LinkedPatch, HotpatchError> {
        let inputs = self.patch_inputs()?;
        let stub = super::stub::create_undefined_symbol_stub(&self.cache, &inputs, anchor_runtime)?;
        let stub_object = self.session_dir.join(format!("stub-{n}.o"));
        std::fs::write(&stub_object, stub).map_err(|err| {
            HotpatchError::io(format!("writing `{}`", stub_object.display()), err)
        })?;
        let output = thin_link::patch_path(&self.target_dir, &self.session, n, self.flavor)?;
        let rlibs = self.modified_rlibs()?;
        let linked = thin_link::thin_link(
            &*self.runner,
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
        let bytes = std::fs::read(&linked.patch).map_err(|err| {
            HotpatchError::io(format!("reading `{}`", linked.patch.display()), err)
        })?;
        let symbols = ImageSymbols::parse(
            &bytes,
            self.target,
            &format!("patch `{}`", linked.patch.display()),
        )?;
        let table = super::jump_table::create_jump_table(&self.cache, &symbols)?;
        Ok(LinkedPatch {
            bytes,
            table,
            symbols,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use frust_devtools_protocol::{HotpatchInfo, RpcError};

    use super::super::layout::LayoutEntry;
    use super::super::seams::SeamInstance;
    use super::super::symbols::ANCHOR_SYMBOL;
    use super::super::symbols::fixtures::{Def, object};
    use super::*;
    use crate::devtools_client::test_server::{self, ApplyReply, FAKE_TOKEN, FakeServer, Script};
    use crate::doctor::FakeEnv;
    use crate::process::Output;

    const TRIPLE: &str = "aarch64-apple-darwin";
    const PID: u32 = 4242;
    const ANCHOR: u64 = 0x1_0000_4000;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-session-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn target() -> Target {
        Target::from_triple(TRIPLE).unwrap()
    }

    /// The fat image: a seam the patches replace, one they never touch.
    fn base_image() -> ImageSymbols {
        let bytes = object(
            target(),
            &[
                Def::Text("pad", 64),
                Def::Text("seam_home", 16),
                Def::Text("seam_other", 16),
                Def::Text(ANCHOR_SYMBOL, 4),
            ],
        );
        ImageSymbols::parse(&bytes, target(), "base").unwrap()
    }

    fn patch_image() -> ImageSymbols {
        let bytes = object(
            target(),
            &[Def::Text("seam_home", 32), Def::Text(ANCHOR_SYMBOL, 4)],
        );
        ImageSymbols::parse(&bytes, target(), "patch").unwrap()
    }

    fn table(entries: &[(&str, u64)]) -> LayoutTable {
        LayoutTable {
            types: entries
                .iter()
                .map(|(path, size)| {
                    (
                        path.to_string(),
                        LayoutEntry {
                            size: *size,
                            align: 4,
                            hash: format!("{size:016x}"),
                        },
                    )
                })
                .collect(),
        }
    }

    /// The seam instances a candidate carries, each `(component, args,
    /// raw symbol)`.
    fn seam_set(entries: &[(&str, &str, &str)]) -> SeamSet {
        SeamSet {
            instances: entries
                .iter()
                .map(|(component, args, symbol)| {
                    (
                        component.to_string(),
                        SeamInstance {
                            component: component.to_string(),
                            args: args.to_string(),
                            symbols: [symbol.to_string()].into_iter().collect(),
                        },
                    )
                })
                .collect(),
        }
    }

    fn home_seam() -> SeamSet {
        seam_set(&[(
            "app::Home",
            "(&app::Home, &mut app::HomeState)",
            "_seam_home",
        )])
    }

    enum FakeCompile {
        Candidate(LayoutTable, SeamSet),
        Failed(Vec<String>),
    }

    /// A builder whose compiles and links are scripted; `calls` records
    /// what the session asked of it.
    struct FakeBuilder {
        graph: Option<WorkspaceGraph>,
        compiles: VecDeque<FakeCompile>,
        calls: Arc<Mutex<Vec<String>>>,
        patch: Vec<u8>,
    }

    impl FakeBuilder {
        fn new(compiles: Vec<FakeCompile>) -> Self {
            Self {
                graph: None,
                compiles: compiles.into(),
                calls: Arc::new(Mutex::new(Vec::new())),
                patch: (0..1000u32).map(|i| (i % 253) as u8).collect(),
            }
        }
    }

    impl PatchBuilder for FakeBuilder {
        fn classify(&self, path: &Path) -> PathClass {
            match &self.graph {
                Some(graph) => graph.classify(path),
                None => PathClass::Replayable {
                    units: [ReplayUnit::lib("app", "app")].into_iter().collect(),
                },
            }
        }

        fn compile(&mut self, units: &BTreeSet<ReplayUnit>) -> Result<Compiled, HotpatchError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("compile {}", units.len()));
            Ok(match self.compiles.pop_front() {
                Some(FakeCompile::Candidate(layouts, seams)) => {
                    Compiled::Candidate { layouts, seams }
                }
                Some(FakeCompile::Failed(diagnostics)) => Compiled::Failed { diagnostics },
                None => Compiled::Candidate {
                    layouts: LayoutTable::default(),
                    seams: home_seam(),
                },
            })
        }

        fn link(&mut self, n: u32, anchor_runtime: u64) -> Result<LinkedPatch, HotpatchError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("link {n} {anchor_runtime:#x}"));
            Ok(LinkedPatch {
                bytes: self.patch.clone(),
                table: JumpTableWire {
                    map: [(0x40, 0x0)].into_iter().collect(),
                    aslr_reference: 0x60,
                    new_base_address: 0x20,
                    ifunc_count: 0,
                },
                symbols: patch_image(),
            })
        }
    }

    fn info() -> HotpatchInfo {
        HotpatchInfo {
            anchor_runtime: ANCHOR,
            pid: PID,
            triple: TRIPLE.into(),
            patches_applied: 0,
            patch_bytes_loaded: 0,
            pending_layout_mismatches: Vec::new(),
        }
    }

    fn hot_script(applies: Vec<ApplyReply>) -> Script {
        Script {
            capabilities: vec![Capability::WidgetTree, Capability::HotPatch],
            info: Some(info()),
            applies: applies.into(),
            ..Script::default()
        }
    }

    /// An applied outcome: `patches_applied` images loaded, `seam_hits`
    /// seam runs and these missed keys.
    fn applied(patches_applied: u32, seam_hits: u64, missed: Vec<MissedKey>) -> ApplyReply {
        ApplyReply::Outcome(PatchOutcome {
            applied: true,
            seam_hits,
            seam_fall_throughs: missed,
            layout_mismatches: Vec::new(),
            patches_applied,
            patch_bytes_loaded: 1000 * u64::from(patches_applied),
        })
    }

    struct Rig {
        session: HotSession,
        server: FakeServer,
        calls: Arc<Mutex<Vec<String>>>,
        dir: PathBuf,
    }

    impl Rig {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        /// The hot-patch requests the app received, beyond the attach.
        fn sent(&self) -> Vec<String> {
            self.server
                .methods()
                .into_iter()
                .skip(2)
                .filter(|m| m == "patch_chunk" || m == "apply_patch")
                .collect()
        }

        fn change(&mut self) -> Outcome {
            self.session
                .on_change(&[PathBuf::from("/w/app/src/lib.rs")])
        }
    }

    /// A session against a fake app scripted by `script`, attached the way
    /// [`start_desktop`] attaches, with `base` seeding the accepted sets.
    fn rig_with(script: Script, builder: FakeBuilder, base: LayoutTable, budget: Budget) -> Rig {
        let server = test_server::spawn(script);
        let app = attach_app(server.addr, Some(FAKE_TOKEN), TRIPLE);
        let target_dir = temp_dir("rig");
        let accepted = AcceptedSets::begin(&target_dir, "session-app", base, home_seam()).unwrap();
        let calls = Arc::clone(&builder.calls);
        let dir = accepted.dir().to_path_buf();
        Rig {
            session: HotSession {
                builder: Box::new(builder),
                app,
                accepted,
                images: vec![base_image()],
                budget,
                restart: None,
                next_patch_id: 1,
            },
            server,
            calls,
            dir,
        }
    }

    fn rig(applies: Vec<ApplyReply>, compiles: Vec<FakeCompile>) -> Rig {
        rig_with(
            hot_script(applies),
            FakeBuilder::new(compiles),
            table(&[("app::HomeState", 4)]),
            Budget::default(),
        )
    }

    fn restart_reason(outcome: Outcome) -> RestartReason {
        match outcome {
            Outcome::RestartRequired(reason) => reason,
            other => panic!("expected a restart, got {other:?}"),
        }
    }

    fn base_key(raw: &str) -> MissedKey {
        MissedKey {
            image: 0,
            link_address: base_image().defined_address(raw).unwrap(),
        }
    }

    /// A front-end drives `on_change` off its UI thread.
    #[test]
    fn a_session_can_move_to_another_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<HotSession>();
    }

    #[test]
    fn an_applied_patch_is_uploaded_in_chunks_applied_and_reported_patched() {
        let mut rig = rig(vec![applied(1, 2, Vec::new())], Vec::new());
        assert_eq!(rig.session.pid(), Some(PID));
        let outcome = rig.change();
        assert!(
            matches!(outcome, Outcome::Patched { components: 2, .. }),
            "{outcome:?}"
        );
        assert_eq!(rig.calls(), vec!["compile 1", "link 1 0x100004000"]);
        assert_eq!(
            rig.server.methods(),
            vec![
                "handshake",
                "hotpatch_info",
                "hotpatch_info",
                "patch_chunk",
                "apply_patch"
            ]
        );
        let patch: Vec<u8> = (0..1000u32).map(|i| (i % 253) as u8).collect();
        assert_eq!(rig.server.uploaded(1), Some(patch));
        let apply = rig.server.requests().pop().unwrap();
        let params: ApplyPatchParams = serde_json::from_value(apply.params).unwrap();
        assert_eq!(
            (
                params.patch_id,
                params.len,
                params.pid,
                params.anchor_runtime,
                params.expected_seams
            ),
            (1, 1000, PID, ANCHOR, 1)
        );
        assert!(outcome.to_string().starts_with("patched in "));
        assert!(outcome.to_string().ends_with(" ms (2 components rebuilt)"));
    }

    #[test]
    fn no_hotpatch_capability_means_a_restart_only_session_that_sends_nothing() {
        let script = Script {
            capabilities: vec![Capability::WidgetTree],
            unsupported_reason: "the devtools token is not OS-sourced".into(),
            ..Script::default()
        };
        let mut rig = rig_with(
            script,
            FakeBuilder::new(Vec::new()),
            LayoutTable::default(),
            Budget::default(),
        );
        assert_eq!(
            rig.session.restart_only_reason(),
            Some("the devtools token is not OS-sourced")
        );
        let reason = restart_reason(rig.change());
        assert_eq!(
            reason,
            RestartReason::HotPatchUnavailable {
                reason: "the devtools token is not OS-sourced".into()
            }
        );
        assert_eq!(rig.server.methods(), vec!["handshake", "hotpatch_info"]);
        assert!(rig.calls().is_empty(), "no thin build: {:?}", rig.calls());
        assert_eq!(rig.session.pid(), None);
    }

    #[test]
    fn a_non_loopback_endpoint_is_sent_nothing() {
        // The fake listens on 127.0.0.1, so the IPv4-mapped IPv6 form of its
        // address reaches it, but the session counts that form as not
        // loopback: nothing may arrive.
        let server = test_server::spawn(hot_script(vec![applied(1, 1, Vec::new())]));
        let mapped = SocketAddr::new(
            std::net::IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped()),
            server.addr.port(),
        );
        let app = attach_app(mapped, Some(FAKE_TOKEN), TRIPLE);
        let AppLink::RestartOnly { reason } = &app else {
            panic!("a non-loopback endpoint must not be attached");
        };
        assert!(reason.contains("not loopback"), "{reason}");
        let builder = FakeBuilder::new(Vec::new());
        let calls = Arc::clone(&builder.calls);
        let accepted = AcceptedSets::begin(
            &temp_dir("non-loopback"),
            "session-app",
            LayoutTable::default(),
            SeamSet::default(),
        )
        .unwrap();
        let mut session = HotSession {
            builder: Box::new(builder),
            app,
            accepted,
            images: vec![base_image()],
            budget: Budget::default(),
            restart: None,
            next_patch_id: 1,
        };
        let reason = restart_reason(session.on_change(&[PathBuf::from("/w/app/src/lib.rs")]));
        assert!(matches!(reason, RestartReason::HotPatchUnavailable { .. }));
        assert!(calls.lock().unwrap().is_empty());
        std::thread::sleep(Duration::from_millis(50));
        assert!(server.methods().is_empty(), "{:?}", server.methods());
    }

    #[test]
    fn a_live_session_whose_endpoint_is_not_loopback_sends_nothing() {
        let mut rig = rig(vec![applied(1, 1, Vec::new())], Vec::new());
        if let AppLink::Live { endpoint, .. } = &mut rig.session.app {
            *endpoint = SocketAddr::from(([192, 0, 2, 7], endpoint.port()));
        }
        let reason = restart_reason(rig.change());
        assert!(
            matches!(reason, RestartReason::EndpointNotLoopback { .. }),
            "{reason:?}"
        );
        assert!(rig.calls().is_empty());
        assert_eq!(rig.server.methods(), vec!["handshake", "hotpatch_info"]);
    }

    #[test]
    fn the_candidate_is_merged_only_on_applied_true() {
        // Applied: the new type joins the accepted set and the file.
        let mut r = rig(
            vec![applied(1, 1, Vec::new())],
            vec![FakeCompile::Candidate(
                table(&[("app::Badge", 4)]),
                home_seam(),
            )],
        );
        assert!(matches!(r.change(), Outcome::Patched { .. }));
        assert!(r.session.accepted().layouts().get("app::Badge").is_some());
        let written = LayoutTable::read(&r.dir.join(LAYOUTS_ACCEPTED_FILE)).unwrap();
        assert_eq!(&written, r.session.accepted().layouts());

        // Applied with an L2 mismatch: refused, nothing merged.
        let mut mismatch = PatchOutcome {
            applied: true,
            seam_hits: 1,
            seam_fall_throughs: Vec::new(),
            layout_mismatches: vec!["app::HomeState: 4 -> 8 bytes".into()],
            patches_applied: 1,
            patch_bytes_loaded: 1000,
        };
        let mut r = rig(
            vec![ApplyReply::Outcome(mismatch.clone())],
            vec![FakeCompile::Candidate(
                table(&[("app::Badge", 4)]),
                home_seam(),
            )],
        );
        assert!(matches!(
            restart_reason(r.change()),
            RestartReason::LayoutChanged { .. }
        ));
        assert!(r.session.accepted().layouts().get("app::Badge").is_none());
        assert!(!r.dir.join(LAYOUTS_ACCEPTED_FILE).exists());

        // Not applied: nothing merged either.
        mismatch.applied = false;
        mismatch.layout_mismatches.clear();
        mismatch.patches_applied = 0;
        let mut r = rig(
            vec![ApplyReply::Outcome(mismatch)],
            vec![FakeCompile::Candidate(
                table(&[("app::Badge", 4)]),
                home_seam(),
            )],
        );
        assert!(matches!(
            restart_reason(r.change()),
            RestartReason::PatchRefused { .. }
        ));
        assert!(r.session.accepted().layouts().get("app::Badge").is_none());
        assert!(!r.dir.join(LAYOUTS_ACCEPTED_FILE).exists());
    }

    #[test]
    fn a_lost_apply_reply_is_patch_outcome_unknown_and_nothing_more_is_sent() {
        let mut rig = rig(
            vec![ApplyReply::Hangup],
            vec![FakeCompile::Candidate(
                table(&[("app::Badge", 4)]),
                home_seam(),
            )],
        );
        let reason = restart_reason(rig.change());
        assert!(
            matches!(reason, RestartReason::PatchOutcomeUnknown { .. }),
            "{reason:?}"
        );
        assert!(rig.session.accepted().layouts().get("app::Badge").is_none());
        // Sticky: the next change answers the same restart and builds nothing.
        assert_eq!(restart_reason(rig.change()), reason);
        assert_eq!(rig.calls(), vec!["compile 1", "link 1 0x100004000"]);
    }

    #[test]
    fn an_apply_patch_error_reply_is_a_refusal() {
        let mut rig = rig(
            vec![ApplyReply::Error(RpcError::new(
                RpcError::INTERNAL_ERROR,
                "patch not applied: dlopen failed",
            ))],
            Vec::new(),
        );
        let reason = restart_reason(rig.change());
        assert!(
            matches!(&reason, RestartReason::PatchRefused { detail } if detail.contains("dlopen")),
            "{reason:?}"
        );
    }

    #[test]
    fn a_late_l2_mismatch_pending_after_an_applied_patch_restarts_and_sends_nothing_more() {
        let mut script = hot_script(vec![applied(1, 1, Vec::new()), applied(2, 1, Vec::new())]);
        script.pending = [
            Vec::new(),
            Vec::new(),
            vec!["app::HomeState changed layout (4 → 8 bytes)".to_string()],
        ]
        .into();
        let mut rig = rig_with(
            script,
            FakeBuilder::new(Vec::new()),
            table(&[("app::HomeState", 4)]),
            Budget::default(),
        );
        assert!(matches!(rig.change(), Outcome::Patched { .. }));
        let reason = restart_reason(rig.change());
        assert_eq!(
            reason,
            RestartReason::LayoutChanged {
                records: vec!["app::HomeState changed layout (4 → 8 bytes)".into()]
            }
        );
        assert_eq!(
            reason.to_string(),
            "app::HomeState changed layout (4 → 8 bytes); restarting to keep memory safe"
        );
        assert_eq!(rig.sent(), vec!["patch_chunk", "apply_patch"]);
        assert_eq!(
            rig.calls(),
            vec!["compile 1", "link 1 0x100004000", "compile 1"]
        );
        // Nothing further either.
        assert_eq!(restart_reason(rig.change()), reason);
        assert_eq!(rig.sent(), vec!["patch_chunk", "apply_patch"]);
    }

    #[test]
    fn an_applied_false_refusal_carrying_records_is_layout_changed() {
        let mut rig = rig(
            vec![ApplyReply::Outcome(PatchOutcome {
                applied: false,
                seam_hits: 0,
                seam_fall_throughs: Vec::new(),
                layout_mismatches: vec!["app::HomeState changed layout".into()],
                patches_applied: 0,
                patch_bytes_loaded: 0,
            })],
            Vec::new(),
        );
        assert_eq!(
            restart_reason(rig.change()),
            RestartReason::LayoutChanged {
                records: vec!["app::HomeState changed layout".into()]
            }
        );
    }

    #[test]
    fn missed_keys_only_in_the_newest_patch_are_patched() {
        let newest = MissedKey {
            image: 1,
            link_address: patch_image().defined_address("_seam_home").unwrap(),
        };
        let mut rig = rig(vec![applied(1, 3, vec![newest, newest])], Vec::new());
        assert_eq!(
            match rig.change() {
                Outcome::Patched { components, .. } => components,
                other => panic!("{other:?}"),
            },
            3
        );
    }

    #[test]
    fn a_base_image_missed_key_on_a_reported_seam_restarts() {
        let mut rig = rig(
            vec![applied(1, 1, vec![base_key("_seam_home")])],
            Vec::new(),
        );
        let reason = restart_reason(rig.change());
        assert_eq!(
            reason,
            RestartReason::StaleSeamCall {
                image: 0,
                symbol: "_seam_home".into()
            }
        );
        assert_eq!(
            reason.to_string(),
            "stale code in the base image still calls the replaced seam `_seam_home`"
        );
        // The patch is live, so its entries were merged all the same.
        assert!(rig.dir.join(LAYOUTS_ACCEPTED_FILE).exists());
    }

    #[test]
    fn a_base_image_missed_key_on_a_seam_the_patch_does_not_carry_is_benign() {
        let mut rig = rig(
            vec![applied(1, 1, vec![base_key("_seam_other")])],
            Vec::new(),
        );
        assert!(matches!(rig.change(), Outcome::Patched { .. }));
    }

    #[test]
    fn an_older_patch_missed_key_on_a_reported_seam_restarts() {
        let older = MissedKey {
            image: 1,
            link_address: patch_image().defined_address("_seam_home").unwrap(),
        };
        let mut rig = rig(
            vec![applied(1, 1, Vec::new()), applied(2, 1, vec![older])],
            Vec::new(),
        );
        assert!(matches!(rig.change(), Outcome::Patched { .. }));
        assert_eq!(
            restart_reason(rig.change()),
            RestartReason::StaleSeamCall {
                image: 1,
                symbol: "_seam_home".into()
            }
        );
    }

    #[test]
    fn a_missed_key_of_an_unknown_image_fails_closed() {
        let unknown = MissedKey {
            image: 7,
            link_address: 0x10,
        };
        let mut rig = rig(vec![applied(1, 1, vec![unknown])], Vec::new());
        assert_eq!(
            restart_reason(rig.change()),
            RestartReason::UnresolvedMissedKey {
                image: 7,
                link_address: 0x10
            }
        );
    }

    #[test]
    fn no_seam_hit_restarts() {
        let mut rig = rig(vec![applied(1, 0, Vec::new())], Vec::new());
        assert_eq!(restart_reason(rig.change()), RestartReason::NoSeamHit);
    }

    #[test]
    fn over_the_default_patch_count_restarts_before_anything_is_linked() {
        let mut script = hot_script(Vec::new());
        script.info.as_mut().unwrap().patches_applied = DEFAULT_BUDGET_PATCHES;
        let mut rig = rig_with(
            script,
            FakeBuilder::new(Vec::new()),
            LayoutTable::default(),
            Budget::default(),
        );
        // The session tracks one image per patch it applied.
        let base = rig.session.images[0].clone();
        rig.session
            .images
            .extend((0..DEFAULT_BUDGET_PATCHES).map(|_| base.clone()));
        let reason = restart_reason(rig.change());
        assert_eq!(
            reason,
            RestartReason::PatchBudget {
                patches: DEFAULT_BUDGET_PATCHES + 1,
                bytes: 0,
                budget: Budget::default()
            }
        );
        assert_eq!(rig.calls(), vec!["compile 1"]);
        assert!(rig.sent().is_empty());
    }

    #[test]
    fn over_the_default_byte_budget_restarts_before_sending() {
        let mut script = hot_script(Vec::new());
        script.info.as_mut().unwrap().patch_bytes_loaded = DEFAULT_BUDGET_BYTES - 10;
        let mut rig = rig_with(
            script,
            FakeBuilder::new(Vec::new()),
            LayoutTable::default(),
            Budget::default(),
        );
        let reason = restart_reason(rig.change());
        assert!(
            matches!(reason, RestartReason::PatchBudget { patches: 1, bytes, .. } if bytes == DEFAULT_BUDGET_BYTES + 990),
            "{reason:?}"
        );
        assert!(rig.sent().is_empty());
    }

    #[test]
    fn a_hotpatch_section_overrides_the_budget() {
        let section = HotpatchSection {
            patches: Some(1),
            bytes: None,
        };
        let budget = Budget::from_section(Some(&section));
        assert_eq!(
            budget,
            Budget {
                patches: 1,
                bytes: DEFAULT_BUDGET_BYTES
            }
        );
        assert_eq!(Budget::from_section(None), Budget::default());
        let manifest =
            manifest::parse("[app]\nname = \"a\"\norg = \"o\"\n\n[hotpatch]\nbytes = 2048\n")
                .unwrap();
        assert_eq!(
            Budget::from_section(manifest.hotpatch.as_ref()),
            Budget {
                patches: DEFAULT_BUDGET_PATCHES,
                bytes: 2048
            }
        );

        let mut rig = rig_with(
            hot_script(vec![applied(1, 1, Vec::new())]),
            FakeBuilder::new(Vec::new()),
            LayoutTable::default(),
            budget,
        );
        assert!(matches!(rig.change(), Outcome::Patched { .. }));
        let reason = restart_reason(rig.change());
        assert!(
            matches!(reason, RestartReason::PatchBudget { patches: 2, budget: b, .. } if b == budget),
            "{reason:?}"
        );
        assert_eq!(rig.sent(), vec!["patch_chunk", "apply_patch"]);
        assert!(reason.to_string().contains("1 patches"), "{reason}");
    }

    /// A workspace at `/w` whose `app` depends on the non-member path
    /// package `frust-material`.
    fn graph_with_path_dependency() -> WorkspaceGraph {
        const APP: &str = "path+file:///w/app#0.1.0";
        const MATERIAL: &str = "path+file:///x/material#0.6.0";
        let target = |name: &str, kind: &str, src: &str| serde_json::json!({"name": name, "kind": [kind], "crate_types": [kind], "src_path": src});
        let json = serde_json::json!({
            "packages": [
                {"id": APP, "name": "app", "source": null, "manifest_path": "/w/app/Cargo.toml",
                 "targets": [target("app", "lib", "/w/app/src/lib.rs"),
                             target("app", "bin", "/w/app/src/main.rs")]},
                {"id": MATERIAL, "name": "frust-material", "source": null,
                 "manifest_path": "/x/material/Cargo.toml",
                 "targets": [target("frust_material", "lib", "/x/material/src/lib.rs")]},
            ],
            "workspace_members": [APP],
            "resolve": {"nodes": [
                {"id": APP, "deps": [{"name": "frust_material", "pkg": MATERIAL,
                                      "dep_kinds": [{"kind": null, "target": null}]}]},
                {"id": MATERIAL, "deps": []},
            ], "root": APP},
            "workspace_root": "/w",
        });
        WorkspaceGraph::from_metadata(&json.to_string(), "app", None).unwrap()
    }

    #[test]
    fn a_path_dependency_change_restarts_with_no_thin_build() {
        let mut builder = FakeBuilder::new(Vec::new());
        builder.graph = Some(graph_with_path_dependency());
        let mut rig = rig_with(
            hot_script(Vec::new()),
            builder,
            LayoutTable::default(),
            Budget::default(),
        );
        let file = PathBuf::from("/x/material/src/button.rs");
        let reason = restart_reason(rig.session.on_change(std::slice::from_ref(&file)));
        assert_eq!(
            reason,
            RestartReason::PathDependencyChanged {
                package: "frust-material".into(),
                file
            }
        );
        assert!(rig.calls().is_empty(), "no thin build: {:?}", rig.calls());
        assert_eq!(rig.server.methods(), vec!["handshake", "hotpatch_info"]);
    }

    #[test]
    fn a_build_input_change_restarts_with_no_thin_build() {
        let mut builder = FakeBuilder::new(Vec::new());
        builder.graph = Some(graph_with_path_dependency());
        let mut rig = rig_with(
            hot_script(Vec::new()),
            builder,
            LayoutTable::default(),
            Budget::default(),
        );
        let reason = restart_reason(rig.session.on_change(&[PathBuf::from("/w/app/Cargo.toml")]));
        assert!(
            matches!(&reason, RestartReason::BuildInputChanged { package: Some(p), .. } if p == "app"),
            "{reason:?}"
        );
        assert!(rig.calls().is_empty());
    }

    #[test]
    fn an_unaffected_path_is_no_change() {
        let mut builder = FakeBuilder::new(Vec::new());
        builder.graph = Some(graph_with_path_dependency());
        let mut rig = rig_with(
            hot_script(Vec::new()),
            builder,
            LayoutTable::default(),
            Budget::default(),
        );
        assert_eq!(
            rig.session
                .on_change(&[PathBuf::from("/elsewhere/notes.md")]),
            Outcome::NoChange
        );
        assert!(rig.calls().is_empty());
    }

    #[test]
    fn a_compile_error_leaves_the_session_patchable() {
        let mut rig = rig(
            vec![applied(1, 1, Vec::new())],
            vec![FakeCompile::Failed(vec!["error[E0308]".into()])],
        );
        assert_eq!(
            rig.change(),
            Outcome::CompileFailed {
                diagnostics: vec!["error[E0308]".into()]
            }
        );
        assert!(rig.sent().is_empty());
        assert!(matches!(rig.change(), Outcome::Patched { .. }));
    }

    #[test]
    fn a_layout_change_against_an_accepted_patch_is_refused_before_sending() {
        // Patch 1 adds `Badge` (new, passes, merged); patch 2 grows it.
        let mut rig = rig(
            vec![applied(1, 1, Vec::new())],
            vec![
                FakeCompile::Candidate(table(&[("app::Badge", 4)]), home_seam()),
                FakeCompile::Candidate(table(&[("app::Badge", 8)]), home_seam()),
            ],
        );
        assert!(matches!(rig.change(), Outcome::Patched { .. }));
        let reason = restart_reason(rig.change());
        assert_eq!(
            reason,
            RestartReason::LayoutChanged {
                records: vec!["app::Badge changed layout (4 → 8 bytes)".into()]
            }
        );
        assert_eq!(rig.sent(), vec!["patch_chunk", "apply_patch"]);
        assert_eq!(
            rig.calls(),
            vec!["compile 1", "link 1 0x100004000", "compile 1"]
        );
    }

    #[test]
    fn a_state_identity_change_is_refused_before_sending() {
        let changed = seam_set(&[("app::Home", "(&app::Home, &mut app::OtherState)", "_x")]);
        let mut rig = rig(
            Vec::new(),
            vec![FakeCompile::Candidate(LayoutTable::default(), changed)],
        );
        let reason = restart_reason(rig.change());
        assert!(
            matches!(reason, RestartReason::StateTypeChanged { .. }),
            "{reason:?}"
        );
        assert!(rig.sent().is_empty());
        assert_eq!(rig.server.methods(), vec!["handshake", "hotpatch_info"]);
    }

    #[test]
    fn the_session_dir_is_reset_and_the_sets_reseeded_on_every_start() {
        let target_dir = temp_dir("lifecycle");
        let mut first = AcceptedSets::begin(
            &target_dir,
            "session-app",
            table(&[("app::HomeState", 4)]),
            home_seam(),
        )
        .unwrap();
        let dir = first.dir().to_path_buf();
        assert_eq!(dir, hotpatch_root(&target_dir).join("session-app"));
        assert_eq!(
            LayoutTable::read(&dir.join(layout::LAYOUT_BASE_FILE)).unwrap(),
            table(&[("app::HomeState", 4)])
        );
        assert!(!dir.join(LAYOUTS_ACCEPTED_FILE).exists());

        first
            .accept(
                &table(&[("app::Badge", 4), ("app::HomeState", 8)]),
                &seam_set(&[("app::Badge", "(&app::Badge, &mut ())", "_seam_badge")]),
            )
            .unwrap();
        // Merge never replaces an accepted entry.
        assert_eq!(first.layouts().get("app::HomeState").unwrap().size, 4);
        assert!(first.seams().get("app::Badge").is_some());
        let written = LayoutTable::read(&dir.join(LAYOUTS_ACCEPTED_FILE)).unwrap();
        assert_eq!(written, table(&[("app::Badge", 4), ("app::HomeState", 4)]));
        std::fs::write(dir.join("patch-1.dylib"), b"patch").unwrap();

        // A relaunch: new base, fresh dir, sets from the new base only.
        let second = AcceptedSets::begin(
            &target_dir,
            "session-app",
            table(&[("app::HomeState", 8)]),
            SeamSet::default(),
        )
        .unwrap();
        assert_eq!(second.dir(), dir);
        assert!(!dir.join("patch-1.dylib").exists());
        assert!(!dir.join(LAYOUTS_ACCEPTED_FILE).exists());
        assert_eq!(second.layouts(), &table(&[("app::HomeState", 8)]));
        assert!(second.seams().is_empty());
        assert_eq!(
            LayoutTable::read(&dir.join(layout::LAYOUT_BASE_FILE)).unwrap(),
            table(&[("app::HomeState", 8)])
        );
    }

    #[test]
    fn a_relaunched_session_does_not_inherit_accepted_entries() {
        let mut rig1 = rig(
            vec![applied(1, 1, Vec::new())],
            vec![FakeCompile::Candidate(
                table(&[("app::Badge", 4)]),
                home_seam(),
            )],
        );
        assert!(matches!(rig1.change(), Outcome::Patched { .. }));
        assert!(
            rig1.session
                .accepted()
                .layouts()
                .get("app::Badge")
                .is_some()
        );
        let target_dir = rig1.dir.parent().unwrap().parent().unwrap().to_path_buf();
        drop(rig1);
        let relaunched = AcceptedSets::begin(
            &target_dir,
            "session-app",
            table(&[("app::HomeState", 4)]),
            home_seam(),
        )
        .unwrap();
        assert!(relaunched.layouts().get("app::Badge").is_none());
        assert!(!relaunched.dir().join(LAYOUTS_ACCEPTED_FILE).exists());
    }

    // ---- debuginfo precondition ----

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn debuginfo_detail(result: Result<(), HotpatchError>) -> String {
        match result {
            Err(HotpatchError::BuilderUnsupported { detail }) => detail,
            other => panic!("expected BuilderUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn debuginfo_resolves_env_then_cargo_config_then_the_manifest() {
        let root = temp_dir("debuginfo");
        let env = FakeEnv::new();
        // Unset everywhere: cargo's dev default, full debug info.
        check_debuginfo(&env, &root, &root).unwrap();

        write(
            &root.join("Cargo.toml"),
            "[profile.dev]\ndebug = \"line-tables-only\"\n",
        );
        let detail = debuginfo_detail(check_debuginfo(&env, &root, &root));
        assert!(detail.starts_with("debuginfo off"), "{detail}");
        assert!(detail.contains("line-tables-only"), "{detail}");

        write(
            &root.join(".cargo/config.toml"),
            "[profile.dev]\ndebug = 2\n",
        );
        check_debuginfo(&env, &root, &root).unwrap();

        let env = FakeEnv::new().set(DEV_DEBUG_ENV, "1");
        let detail = debuginfo_detail(check_debuginfo(&env, &root, &root));
        assert!(detail.starts_with("debuginfo off"), "{detail}");

        for full in ["2", "true", "full"] {
            let env = FakeEnv::new().set(DEV_DEBUG_ENV, full);
            check_debuginfo(&env, &root, &root).unwrap();
        }
        for off in ["0", "false", "none", "limited", "line-directives-only"] {
            let env = FakeEnv::new().set(DEV_DEBUG_ENV, off);
            assert!(
                debuginfo_detail(check_debuginfo(&env, &root, &root)).starts_with("debuginfo off")
            );
        }

        write(
            &root.join(".cargo/config.toml"),
            "[profile.dev]\ndebug = false\n",
        );
        let detail = debuginfo_detail(check_debuginfo(&FakeEnv::new(), &root, &root));
        assert!(detail.contains("config.toml"), "{detail}");
    }

    /// One recorded invocation: command, arguments, added environment, cwd.
    type Call = (String, Vec<String>, Vec<(String, String)>, Option<PathBuf>);

    /// Scripted answers for [`start_desktop`]'s toolchain calls, recording
    /// every invocation with its environment.
    struct ScriptedRunner {
        runs: HashMap<String, Output>,
        streaming: Output,
        calls: Mutex<Vec<Call>>,
    }

    impl ScriptedRunner {
        fn invocations(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(cmd, args, _, _)| format!("{cmd} {}", args.join(" ")))
                .collect()
        }

        fn call(&self, cmd: &str) -> (Vec<String>, Vec<(String, String)>, Option<PathBuf>) {
            let calls = self.calls.lock().unwrap();
            let (_, args, env, cwd) = calls
                .iter()
                .find(|(c, args, _, _)| c == cmd && args.first().is_some_and(|a| a == "rustc"))
                .expect("a recorded call")
                .clone();
            (args, env, cwd)
        }
    }

    impl ProcessRunner for ScriptedRunner {
        fn run(&self, cmd: &str, args: &[&str]) -> anyhow::Result<Output> {
            self.calls.lock().unwrap().push((
                cmd.to_string(),
                args.iter().map(|a| a.to_string()).collect(),
                Vec::new(),
                None,
            ));
            let key = format!("{cmd} {}", args.join(" "));
            self.runs
                .iter()
                .find(|(prefix, _)| key.starts_with(prefix.as_str()))
                .map(|(_, output)| output.clone())
                .ok_or_else(|| anyhow::anyhow!("unscripted `{key}`"))
        }

        fn run_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
            on_line: &mut dyn FnMut(&str),
        ) -> anyhow::Result<Output> {
            self.calls.lock().unwrap().push((
                cmd.to_string(),
                args.iter().map(|a| a.to_string()).collect(),
                env.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                cwd.map(Path::to_path_buf),
            ));
            for line in self.streaming.stdout.lines() {
                on_line(line);
            }
            Ok(self.streaming.clone())
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

    const RUSTC_VV: &str = "rustc 1.98.1 (abc 2026-09-01)\nbinary: rustc\nhost: aarch64-apple-darwin\nrelease: 1.98.1\n";

    /// A one-package project under a temp dir with its target dir beside
    /// it, and a runner answering `cargo metadata` and `rustc -vV`.
    fn project(tag: &str, streaming: Output) -> (PathBuf, PathBuf, Arc<ScriptedRunner>) {
        let base = temp_dir(tag);
        let root = base.join("my-app");
        let target_dir = base.join("target");
        std::fs::create_dir_all(&root).unwrap();
        const ID: &str = "path+file:///p/my-app#0.1.0";
        let metadata = serde_json::json!({
            "packages": [{"id": ID, "name": "my-app", "source": null,
                "manifest_path": root.join("Cargo.toml"),
                "targets": [
                    {"name": "my_app", "kind": ["cdylib", "staticlib", "rlib"],
                     "crate_types": ["cdylib", "staticlib", "rlib"],
                     "src_path": root.join("src/lib.rs")},
                    {"name": "my-app", "kind": ["bin"], "crate_types": ["bin"],
                     "src_path": root.join("src/main.rs")},
                ]}],
            "workspace_members": [ID],
            "resolve": {"nodes": [{"id": ID, "deps": []}], "root": ID},
            "workspace_root": root,
            "target_directory": target_dir,
        });
        let ok = |stdout: String| Output {
            success: true,
            stdout,
            stderr: String::new(),
        };
        let runner = ScriptedRunner {
            runs: [
                ("cargo metadata".to_string(), ok(metadata.to_string())),
                ("rustc -vV".to_string(), ok(RUSTC_VV.to_string())),
            ]
            .into_iter()
            .collect(),
            streaming,
            calls: Mutex::new(Vec::new()),
        };
        (root, target_dir, Arc::new(runner))
    }

    fn debug_info() -> BuildInfo {
        BuildInfo {
            mode: BuildMode::Debug,
            flavor: None,
            defines: HashMap::new(),
            build_name: None,
            build_number: None,
        }
    }

    fn start(
        runner: &Arc<ScriptedRunner>,
        env: &FakeEnv,
        root: &Path,
        info: &BuildInfo,
        lines: &mut Vec<String>,
    ) -> StartError {
        let host = SessionHost {
            runner: Arc::clone(runner) as Arc<dyn ProcessRunner + Send + Sync>,
            env,
            frust_exe: PathBuf::from("/opt/frust/bin/frust"),
        };
        let request = DesktopStart {
            root,
            info,
            package: "my-app",
            bin: None,
        };
        match start_desktop(&host, &request, &mut |line| lines.push(line.to_string())) {
            Ok(_) => panic!("the scripted runner links nothing; start must stop"),
            Err(err) => err,
        }
    }

    #[test]
    fn dev_debug_zero_in_the_environment_refuses_before_any_build() {
        let (root, _, runner) = project("ci-debug", Output::default());
        let env = FakeEnv::new().set(DEV_DEBUG_ENV, "0");
        let err = start(&runner, &env, &root, &debug_info(), &mut Vec::new());
        match err {
            StartError::RestartRequired(RestartReason::BuilderUnsupported { detail }) => {
                assert!(detail.starts_with("debuginfo off"), "{detail}");
                assert!(detail.contains(DEV_DEBUG_ENV), "{detail}");
            }
            other => panic!("expected debuginfo off, got {other:?}"),
        }
        let invocations = runner.invocations();
        assert!(
            invocations
                .iter()
                .all(|call| call.starts_with("cargo metadata")),
            "nothing but metadata may run: {invocations:?}"
        );
    }

    #[test]
    fn a_profile_or_release_build_is_never_hot() {
        let (root, _, runner) = project("profile", Output::default());
        let mut info = debug_info();
        info.mode = BuildMode::Profile;
        let err = start(&runner, &FakeEnv::new(), &root, &info, &mut Vec::new());
        assert!(
            matches!(
                err,
                StartError::RestartRequired(RestartReason::BuilderUnsupported { .. })
            ),
            "{err:?}"
        );
        assert!(runner.invocations().is_empty());
    }

    #[test]
    fn the_fat_build_runs_cargo_rustc_with_the_capture_and_link_env() {
        let (root, target_dir, runner) = project(
            "fat-argv",
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let mut info = debug_info();
        info.defines.insert("APP_FLAVOR".into(), "dev".into());
        let err = start(&runner, &FakeEnv::new(), &root, &info, &mut Vec::new());
        // The scripted build links nothing, so reading its link args fails.
        assert!(
            matches!(&err, StartError::RestartRequired(RestartReason::BuilderUnsupported { detail }) if detail.contains("link")),
            "{err:?}"
        );

        let (args, env, cwd) = runner.call("cargo");
        assert_eq!(
            args,
            [
                "rustc",
                "-p",
                "my-app",
                "--bin",
                "my-app",
                "--message-format",
                "json-diagnostic-rendered-ansi",
                "--features",
                "frust/perf-trace",
                "--features",
                "frust/devtools",
                "--features",
                "frust/hotpatch",
                "--",
                "-Csave-temps=true",
                "-Clink-dead-code",
                "-Clinker=/opt/frust/bin/frust",
            ]
        );
        assert_eq!(cwd.as_deref(), Some(root.as_path()));

        let scope = ScopeInputs {
            tip: "my-app".into(),
            triple: TRIPLE.into(),
            profile: "dev".into(),
            features: vec![
                "frust/perf-trace".into(),
                "frust/devtools".into(),
                "frust/hotpatch".into(),
            ],
            rustflags: Vec::new(),
            rustc_version: RUSTC_VV.trim().to_string(),
        };
        let scope_dir = prepare_scope_dir(&target_dir, &scope).unwrap();
        let fat_dir = hotpatch_root(&target_dir)
            .join("fat")
            .join(scope.dir_name().unwrap());
        let render = |path: PathBuf| path.to_string_lossy().into_owned();
        let pair = |k: &str, v: String| (k.to_string(), v);
        assert_eq!(
            env,
            vec![
                pair("APP_FLAVOR", "dev".into()),
                pair("RUSTC_WORKSPACE_WRAPPER", "/opt/frust/bin/frust".into()),
                pair("FRUST_HOTPATCH_CAPTURE", render(scope_dir.clone())),
                pair("FRUST_HOTPATCH_LINK", "no-link".into()),
                pair(
                    "FRUST_HOTPATCH_LINK_ARGS_FILE",
                    render(fat_dir.join("link-args.json"))
                ),
                pair(
                    "FRUST_HOTPATCH_LINK_ERR_FILE",
                    render(fat_dir.join("link-err.txt"))
                ),
            ]
        );
        assert!(scope_dir.is_dir());
        assert_eq!(
            runner.invocations()[..2],
            [
                format!(
                    "cargo metadata --format-version 1 --manifest-path {}",
                    root.join("Cargo.toml").display()
                ),
                "rustc -vV".to_string()
            ]
        );
    }

    #[test]
    fn a_failed_fat_build_reports_its_rendered_errors() {
        let message = serde_json::json!({
            "reason": "compiler-message",
            "message": {"level": "error", "rendered": "error[E0425]: cannot find value `x`\n"},
        });
        let warning = serde_json::json!({
            "reason": "compiler-message",
            "message": {"level": "warning", "rendered": "warning: unused\n"},
        });
        let (root, _, runner) = project(
            "fat-fails",
            Output {
                success: false,
                stdout: format!("{message}\n{warning}\nnot json"),
                stderr: "error: could not compile `my-app`".into(),
            },
        );
        let mut lines = Vec::new();
        let err = start(&runner, &FakeEnv::new(), &root, &debug_info(), &mut lines);
        match err {
            StartError::FatBuildFailed { diagnostics } => assert_eq!(
                diagnostics,
                vec![
                    "error[E0425]: cannot find value `x`\n".to_string(),
                    "error: could not compile `my-app`".to_string()
                ]
            ),
            other => panic!("expected FatBuildFailed, got {other:?}"),
        }
        assert_eq!(
            lines,
            vec![
                "error[E0425]: cannot find value `x`",
                "warning: unused",
                "not json"
            ]
        );
    }

    #[test]
    fn fat_build_command_spells_the_capture_and_link_contract() {
        let link = LinkAction {
            mode: LinkMode::NoLink,
            args_file: PathBuf::from("/t/fat/link-args.json"),
            err_file: None,
        };
        let fat = fat_build_command(
            "app",
            "app",
            &["frust/hotpatch"],
            &[("APP_FLAVOR".to_string(), "dev".to_string())],
            Path::new("/bin/frust"),
            Path::new("/t/scope"),
            &link,
        );
        assert_eq!(fat.program, "cargo");
        assert_eq!(
            fat.args.join(" "),
            "rustc -p app --bin app --message-format json-diagnostic-rendered-ansi --features \
             frust/hotpatch -- -Csave-temps=true -Clink-dead-code -Clinker=/bin/frust"
        );
        assert_eq!(
            fat.env,
            vec![
                ("APP_FLAVOR".to_string(), "dev".to_string()),
                (
                    "RUSTC_WORKSPACE_WRAPPER".to_string(),
                    "/bin/frust".to_string()
                ),
                ("FRUST_HOTPATCH_CAPTURE".to_string(), "/t/scope".to_string()),
                ("FRUST_HOTPATCH_LINK".to_string(), "no-link".to_string()),
                (
                    "FRUST_HOTPATCH_LINK_ARGS_FILE".to_string(),
                    "/t/fat/link-args.json".to_string()
                ),
            ]
        );
    }

    #[test]
    fn link_line_helpers_find_member_rlibs_and_tip_objects() {
        let graph = graph_with_path_dependency();
        let args: Vec<String> = [
            "/t/rustcX/symbols.o",
            "/t/debug/deps/app-1.app.a-cgu.0.rcgu.o",
            "/t/debug/deps/libapp-77.rlib",
            "/t/debug/deps/libapp_extra-88.rlib",
            "/t/debug/deps/libfrust_material-9.rlib",
            "/rustlib/libstd-1.rlib",
        ]
        .map(str::to_string)
        .to_vec();
        let rlibs = member_rlibs(&args, &graph);
        assert_eq!(
            rlibs,
            [(
                ReplayUnit::lib("app", "app"),
                PathBuf::from("/t/debug/deps/libapp-77.rlib")
            )]
            .into_iter()
            .collect()
        );
        assert_eq!(
            tip_objects(&args),
            vec![PathBuf::from("/t/debug/deps/app-1.app.a-cgu.0.rcgu.o")]
        );
        assert_eq!(replayable_crates(&graph), vec!["app".to_string()]);
    }

    #[test]
    fn tip_objects_without_dwarf_are_skipped_unless_none_carries_any() {
        let dir = temp_dir("typed-objects");
        let plain = dir.join("app-1.app.a-cgu.0.rcgu.o");
        std::fs::write(&plain, object(target(), &[Def::Text("main", 8)])).unwrap();
        assert_eq!(
            typed_objects(&[], &["app".into()]).unwrap(),
            Vec::<PathBuf>::new()
        );
        let detail = match typed_objects(std::slice::from_ref(&plain), &["app".into()]) {
            Err(HotpatchError::BuilderUnsupported { detail }) => detail,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert!(detail.contains("carries DWARF"), "{detail}");
    }

    #[test]
    fn toolchain_helpers_read_the_host_triple_rustflags_and_linker() {
        assert_eq!(host_triple(RUSTC_VV).unwrap(), TRIPLE);
        assert!(host_triple("rustc 1.0").is_err());
        let env = FakeEnv::new().set("RUSTFLAGS", "-C  target-cpu=native");
        assert_eq!(rustflags(&env), vec!["-C", "target-cpu=native"]);
        let env = env.set("CARGO_ENCODED_RUSTFLAGS", "-Cfoo\u{1f}-Cbar");
        assert_eq!(rustflags(&env), vec!["-Cfoo", "-Cbar"]);
        let env = FakeEnv::new().set("CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER", "/usr/bin/clang");
        assert_eq!(
            custom_linker(&env, TRIPLE),
            Some(PathBuf::from("/usr/bin/clang"))
        );
        assert_eq!(custom_linker(&FakeEnv::new(), TRIPLE), None);
        let args: Vec<String> = [
            "--out-dir",
            "/t/deps",
            "-C",
            "extra-filename=-ab12",
            "-Cdebuginfo=2",
        ]
        .map(str::to_string)
        .to_vec();
        assert_eq!(flag_value(&args, "--out-dir").as_deref(), Some("/t/deps"));
        assert_eq!(
            codegen_value(&args, "extra-filename").as_deref(),
            Some("-ab12")
        );
        assert_eq!(codegen_value(&args, "debuginfo").as_deref(), Some("2"));
        assert_eq!(codegen_value(&args, "opt-level"), None);
    }

    #[test]
    fn every_restart_reason_renders_a_reason_line() {
        let reasons = [
            RestartReason::BuildInputChanged {
                package: None,
                file: PathBuf::from("/w/build.rs"),
            },
            RestartReason::HotPatchUnavailable { reason: "x".into() },
            RestartReason::PatchOutcomeUnknown {
                detail: "closed".into(),
            },
            RestartReason::UnresolvedMissedKey {
                image: 2,
                link_address: 0x40,
            },
            RestartReason::BuilderUnsupported {
                detail: "debuginfo off".into(),
            },
        ];
        let rendered: Vec<String> = reasons
            .into_iter()
            .map(|r| Outcome::RestartRequired(r).to_string())
            .collect();
        assert_eq!(
            rendered,
            vec![
                "restart required: `/w/build.rs` is a build input; a full rebuild picks it up",
                "restart required: hot patching is unavailable for this app: x",
                "restart required: the app's answer to the patch was lost (closed); the patch may be live",
                "restart required: a missed seam key (image 2, 0x40) maps to no known symbol",
                "restart required: hot-patch builder unsupported: debuginfo off",
            ]
        );
    }
}
