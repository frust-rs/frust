//! The hot-patch session: one running desktop app process, built fat, with
//! every later source change either offered to it as a patch or answered
//! with a restart.
//!
//! **Start** ([`start_desktop`], Debug builds only). The workspace graph is
//! read for the host triple the fat build compiles for (`cargo metadata
//! --filter-platform`), so a dependency gated to another platform is no edge
//! of it. The dev profile's resolved debuginfo is checked next
//! ([`check_debuginfo`]): the layout gate reads DWARF types, so anything short
//! of full debug info fails closed before any build. The fat build is then
//! `cargo rustc` with the Debug hot-session features, `-Csave-temps=true
//! -Clink-dead-code -Clinker=<frust>`, `frust` as the workspace wrapper and
//! the link step intercepted ([`fat_build_command`]); the builder links the
//! fat image itself, writes `layout-base.json` and seeds the accepted-layout
//! and accepted-seam sets from it ([`AcceptedSets`]), builds the symbol cache,
//! spawns that image directly (never `cargo run`), reads the devtools
//! discovery line from its output and connects with the token. An app that
//! does not advertise `HotPatch`, or whose endpoint is not loopback, gives a
//! restart-only session carrying the reason.
//!
//! **Change** ([`HotSession::on_change`]). Changed paths are classified: a
//! file of a path dependency outside the workspace whose lib the fat build
//! did not capture, or a build input, is a restart with no thin build. (A
//! desktop fat build captures local non-members too,
//! [`capture::prepare_scope_dir_for`]; each captured one is a replay unit,
//! [`WorkspaceGraph::replay_non_members`], whose types the layout gate
//! covers like a member's, so an edit to it replays it and its dependents
//! up to the tip. The Android and iOS-simulator starts do not capture
//! them.) Otherwise the changed units are replayed, the
//! candidate's layout table and seam instances are checked against the
//! accepted sets, `hotpatch_info`'s pending layout-mismatch records and the
//! patch budget are read, and only then is the stub built against the
//! process's runtime anchor, the patch thin-linked, uploaded in chunks and
//! applied — or, when the app advertises `patch_file_hand_off`, restricted
//! to `0600` and named on `apply_patch` by absolute path and SHA-256 instead
//! of uploaded (the session is loopback-only, so the app reads the very file
//! the host wrote). The jump table is uploaded in chunks either way
//! (`table_chunk`, `DevtoolsClient::upload_table`): a large app's table is
//! several MB, past the devtools request line cap. A device session (Android, through `adb forward`;
//! [`super::android`]) always uploads, and uploads a stripped copy of the
//! patch: the app is not on this host, and the host alone reads the
//! patch's symbols, from the unstripped image. `len`, the byte budget and
//! any digest describe the bytes the app receives. One
//! patch is in flight at a time, and nothing is ever sent to a restart-only
//! session, to a non-loopback endpoint, with pending records or over
//! budget.
//!
//! **Accepted sets.** A candidate's layout and seam entries are merged only
//! when its `PatchOutcome` reports `applied: true` with no layout mismatch;
//! each merge rewrites `layouts-accepted.json` in the session dir for
//! diagnosis (never read back). Every start resets the session dir and
//! reseeds the sets from the new base, so a relaunch is a new session.
//!
//! **Ungated objects.** A patch links every rlib of the cumulative modified
//! set, so no object may reach a link before its DWARF passed the layout
//! gate in a candidate the session then accepted. Every object a replay
//! produces stays in the builder's ungated set (`Ungated`) — through a
//! round that fails on a later unit or on the image unit — and each
//! candidate's layout table is extracted from that whole set, not from the
//! last round's objects alone, by the base table's input rule (a
//! replayable non-member's rlib holding no code or data is left out of
//! both, `gated_inputs`). Only the session's acceptance of an applied
//! patch (`PatchBuilder::accepted`) empties it.
//!
//! **Outcome.** A reply lost after `apply_patch` was sent is
//! `PatchOutcomeUnknown` (the patch may be live). A missed seam key in the
//! newest patch is a benign patch-image caller; one in the base or an older
//! patch is mapped through that image's symbol table and requires a restart
//! when it names a seam instance the patch carries. No seam hit at all is a
//! restart too. Once a session has answered a restart, it answers the same
//! restart to every later change and sends nothing more.
//!
//! **Windows (`pc-windows-msvc`).** The same session, with PE arms: the
//! fat image is `<bin>.exe`, linked by `rust-lld -flavor link`
//! ([`fat_link::flavor_linker_program`]) with the archive under
//! `/WHOLEARCHIVE:`; the symbol cache and anchor come from the exe's own
//! PDB ([`super::pe`]) and the base layout table from that PDB's type
//! records ([`super::pdb_layout`]), not from DWARF, and seam instances
//! from the COFF objects' symbol names. Per change, the replay and stub
//! are the DWARF path's (a CRT link marker such as `_fltused` is left to
//! the patch's own link, [`super::pe::CRT_LINK_MARKERS`]), but the
//! candidate's layout table can only be read once the patch is linked: the
//! thin link writes `patch-<n>.dll` and its `patch-<n>.pdb`, the table is
//! extracted from that PDB (`PatchBuilder::linked_layouts`) and checked
//! against the accepted sets right after the link, before anything is sent
//! — so a refused patch costs one link on Windows. The app's `HotPatch`
//! capability decides whether a patch is ever sent. See
//! `docs/CLI_ARCHITECTURE.md`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::convert::Infallible;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub use frust_devtools_protocol::redact_discovery_token as redact_discovery_line;
use frust_devtools_protocol::{
    ApplyPatchParams, Capability, Discovery, JumpTableWire, MissedKey, PatchFile, PatchOutcome,
    parse_discovery_line, parse_failure_line, redact_discovery_token,
};

use crate::build_info::{BuildInfo, BuildMode};
use crate::desktop_run;
use crate::devtools_client::{
    DevtoolsClient, DevtoolsRpcError, RequestTooLarge, is_not_supported, sha256_hex,
};
use crate::doctor::{EnvLookup, RealEnv};
use crate::manifest::{self, HotpatchSection};
use crate::process::{ProcessRunner, StreamHandle, TryRecvError};

use super::capture::{
    self, CAPTURE_ENV, RecordKey, RustcRecord, ScopeInputs, TargetKind, WrapperSetup, load_records,
    prepare_scope_dir_for,
};
use super::fat_link::{self, FatLinkRequest, LinkerFlavor};
use super::graph::{PathClass, ReplayUnit, WorkspaceGraph};
use super::layout::{self, LayoutTable};
use super::link_intercept::{LinkAction, LinkMode, linker_arg, read_link_args};
use super::pdb_layout;
use super::replay::{
    STRIPPED_ENV, parse_notifications, replay_args_image, replay_env, replay_units,
};
use super::seams::{self, SeamSet};
use super::symbols::{Format, ImageSymbols, SymbolCache, Target};
use super::thin_link::{self, ThinLinkRequest};
use super::{HotpatchError, hotpatch_root};

/// The accepted-layout set after each merge, written beside
/// `layout-base.json` for diagnosis and never read back.
pub const LAYOUTS_ACCEPTED_FILE: &str = "layouts-accepted.json";

/// One [`PatchTimings`] line per attempted patch, appended in the session
/// directory: the host half of the patch-latency instrument (the app logs
/// the matching `frust-hotpatch: apply timings` line).
pub const PATCH_TIMINGS_FILE: &str = "patch-timings.log";

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
pub(super) const FAT_DIR: &str = "fat";

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
    /// A file of a path package outside the workspace changed whose lib the
    /// fat build did not capture (an Android or iOS-simulator session, a
    /// proc macro, a package without a lib); only a fat rebuild picks it up.
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
    /// A devtools call failed before `apply_patch` reached the app (an
    /// upload, or an `apply_patch` line refused unsent); nothing was applied.
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

/// Where one patch attempt spent its time on the host, stage by stage: the
/// permanent instrument for patch-latency regressions. A session records one
/// for every change that reached the compile ([`HotSession::last_timings`])
/// and appends its [`Display`](fmt::Display) line — `hot patch timings:
/// outcome=… compile=…ms gate=…ms …` — to [`PATCH_TIMINGS_FILE`] in the
/// session directory. Taking it costs a few `Instant` reads per patch. A
/// stage the attempt never reached reads 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PatchTimings {
    /// The replays of the changed units' captured rustc invocations: the
    /// thin compile, the tip's intercepted link included.
    pub compile: Duration,
    /// The candidate's gates read from its objects: the layout table's DWARF
    /// and the seam instances.
    pub gate: Duration,
    /// The accepted-set check and the `hotpatch_info` round trip.
    pub check: Duration,
    /// The undefined-symbol stub over the patch's inputs.
    pub symbols: Duration,
    /// The thin link (and, on Windows, the patch PDB's layout table).
    pub link: Duration,
    /// Reading the linked patch's symbols and building its jump table.
    pub table: Duration,
    /// Stripping the upload copy a device is sent.
    pub strip: Duration,
    /// Sending the patch bytes (`patch_chunk`s; 0 on a file hand-off).
    pub upload_patch: Duration,
    /// Sending the jump table (`table_chunk`s).
    pub upload_table: Duration,
    /// `apply_patch` until the app's answer, which follows its next frame.
    pub apply: Duration,
    /// The whole attempt, from the change to the answer.
    pub total: Duration,
    /// The bytes sent or named (the stripped copy on a device).
    pub patch_bytes: u64,
    /// The jump table's entries.
    pub table_entries: u64,
}

impl PatchTimings {
    /// `self` with the stages a [`PatchBuilder`] times taken from `builder`.
    fn with_builder(self, builder: PatchTimings) -> Self {
        Self {
            compile: builder.compile,
            gate: builder.gate,
            symbols: builder.symbols,
            link: builder.link,
            table: builder.table,
            strip: builder.strip,
            ..self
        }
    }
}

impl fmt::Display for PatchTimings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = |d: Duration| d.as_millis();
        write!(
            f,
            "compile={}ms gate={}ms check={}ms symbols={}ms link={}ms table={}ms strip={}ms \
             upload_patch={}ms upload_table={}ms apply={}ms total={}ms patch_bytes={} \
             table_entries={}",
            ms(self.compile),
            ms(self.gate),
            ms(self.check),
            ms(self.symbols),
            ms(self.link),
            ms(self.table),
            ms(self.strip),
            ms(self.upload_patch),
            ms(self.upload_table),
            ms(self.apply),
            ms(self.total),
            self.patch_bytes,
            self.table_entries
        )
    }
}

/// The one-word outcome a [`PATCH_TIMINGS_FILE`] line starts with.
fn outcome_word(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Patched { .. } => "patched",
        Outcome::NoChange => "no_change",
        Outcome::CompileFailed { .. } => "compile_failed",
        Outcome::RestartRequired(_) => "restart",
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
/// capture variable naming the wrapper's executable and scope, then
/// `link`'s variables, then [`capture::AMBIENT_ENV`] listing the wrapper's
/// ambient names (the host's own environment) plus every name added here —
/// the environment cargo starts from, which the wrapper subtracts to find
/// cargo's own additions. The `--` flags reach only the tip bin.
pub fn fat_build_command(
    package: &str,
    bin: &str,
    features: &[&str],
    defines: &[(String, String)],
    wrapper: WrapperSetup<'_>,
    link: &LinkAction,
) -> FatBuild {
    let WrapperSetup {
        frust_exe,
        scope_dir,
        ambient_names,
    } = wrapper;
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
    let ambient = capture::ambient_env_var(
        ambient_names
            .iter()
            .map(String::as_str)
            .chain(env.iter().map(|(name, _)| name.as_str()))
            .chain([capture::AMBIENT_ENV]),
    );
    env.push(ambient);
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

    /// The host gates: every component keeps its `State` identity, then every
    /// type the candidate shares with the accepted set keeps its layout. Passing returns the candidate's seam instances.
    pub fn check(&self, layouts: &LayoutTable, seams: SeamSet) -> Result<SeamSet, RestartReason> {
        // The identity check runs first: a changed `State` type also changes
        // the hashed member type of its component widget, so after the layout
        // diff the identity reason would be unreachable on a DWARF target.
        let report = seams::check(seams, &self.seams);
        if !report.changed.is_empty() {
            return Err(RestartReason::StateTypeChanged {
                changes: report.changed.iter().map(ToString::to_string).collect(),
            });
        }
        let changed = layout::diff(layouts, &self.layouts);
        if !changed.is_empty() {
            return Err(RestartReason::LayoutChanged {
                records: changed.iter().map(ToString::to_string).collect(),
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
    /// The candidate's layout table (of every object compiled since the
    /// last accepted patch: the builder's [`Ungated`] set) and seam
    /// instances (of everything the patch will carry).
    Candidate {
        layouts: LayoutTable,
        seams: SeamSet,
    },
}

/// A linked patch, ready to send.
struct LinkedPatch {
    /// The file the app is sent or named: the linked `patch-<n>` under the
    /// session dir, or its stripped upload copy beside it.
    path: PathBuf,
    /// `path`'s contents.
    bytes: Vec<u8>,
    table: JumpTableWire,
    /// The symbols of the linked, unstripped image.
    symbols: ImageSymbols,
}

/// Reads the thin-linked image at `patch`, builds its jump table against
/// `cache` and, when `upload_strip` names a strip tool, writes the stripped
/// copy the app is sent ([`super::android::strip_for_upload`]). Symbols and
/// table always come from `patch` itself (a PE patch's from its own PDB);
/// without a strip tool the patch is sent as linked.
fn read_linked(
    runner: &dyn ProcessRunner,
    cache: &SymbolCache,
    target: Target,
    patch: PathBuf,
    upload_strip: Option<&Path>,
    timings: &mut PatchTimings,
) -> Result<LinkedPatch, HotpatchError> {
    let tabling = Instant::now();
    let read = |path: &Path| {
        std::fs::read(path)
            .map_err(|err| HotpatchError::io(format!("reading `{}`", path.display()), err))
    };
    let bytes = read(&patch)?;
    let symbols = if target.format() == Format::Pe {
        ImageSymbols::load(&patch, target)?
    } else {
        ImageSymbols::parse(&bytes, target, &format!("patch `{}`", patch.display()))?
    };
    let table = super::jump_table::create_jump_table(cache, &symbols)?;
    timings.table = tabling.elapsed();
    let stripping = Instant::now();
    let (path, bytes) = match upload_strip {
        None => (patch, bytes),
        Some(strip) => {
            let upload = super::android::strip_for_upload(runner, strip, &patch)?;
            let bytes = read(&upload)?;
            (upload, bytes)
        }
    };
    timings.strip = stripping.elapsed();
    Ok(LinkedPatch {
        path,
        bytes,
        table,
        symbols,
    })
}

/// Prepares the loopback hand-off of the patch at `path` whose bytes are
/// `bytes`: restricts the file to its owner (`0600`, which the app requires)
/// and names it by absolute path and SHA-256. `None` when the path cannot be
/// put on the wire (not UTF-8): the session then uploads the patch in chunks.
/// Public so the CI canary prepares its patch exactly as a session does.
pub fn hand_off(path: &Path, bytes: &[u8]) -> Result<Option<PatchFile>, HotpatchError> {
    thin_link::restrict_to_owner(path)?;
    let absolute = std::path::absolute(path)
        .map_err(|err| HotpatchError::io(format!("resolving `{}`", path.display()), err))?;
    Ok(absolute.to_str().map(|path| PatchFile {
        path: path.to_string(),
        sha256: sha256_hex(bytes),
    }))
}

/// The build half of a session: classification, replay, thin link. A
/// trait so the session's decisions are testable without a toolchain.
trait PatchBuilder: Send {
    fn classify(&self, path: &Path) -> PathClass;
    fn compile(&mut self, units: &BTreeSet<ReplayUnit>) -> Result<Compiled, HotpatchError>;
    /// Links patch number `n` against the process whose anchor is at
    /// `anchor_runtime`.
    fn link(&mut self, n: u32, anchor_runtime: u64) -> Result<LinkedPatch, HotpatchError>;
    /// The candidate's layout table read from the patch [`link`] just
    /// produced, taken once: a PE patch's own PDB
    /// ([`pdb_layout`]), the only place its type records are readable. The
    /// session gates it before anything is sent. `None` (the default) when
    /// the table came with [`Compiled::Candidate`], before the link, as on
    /// every DWARF target.
    ///
    /// [`link`]: PatchBuilder::link
    fn linked_layouts(&mut self) -> Option<LayoutTable> {
        None
    }
    /// The session accepted the last candidate: its patch reported
    /// `applied: true` with no layout mismatch and its entries were merged
    /// into the accepted sets. Every object compiled so far has now passed
    /// the gates, so the builder empties its [`Ungated`] set. Called once
    /// per accepted patch and on no other path (a refused candidate, a
    /// failed link, `applied: false`, a lost reply or a layout-mismatch
    /// record leave the set as it is).
    fn accepted(&mut self);
    /// The stages this builder timed since the last call ([`PatchTimings`]'s
    /// `compile`, `gate`, `symbols`, `link`, `table` and `strip`), resetting
    /// them. Zero by default.
    fn take_timings(&mut self) -> PatchTimings {
        PatchTimings::default()
    }
}

/// The objects compiled since the session last accepted a patch, by unit:
/// a lib's rlib, the image unit's typed objects. A patch links every one of
/// them, so each candidate's layout table is extracted from all of them. A
/// unit enters on its successful replay — whether or not the rest of its
/// round compiled — replacing its earlier entry; the set is emptied only
/// by [`Ungated::accept`], which the session reaches through
/// [`PatchBuilder::accepted`].
#[derive(Debug, Default)]
struct Ungated {
    objects: BTreeMap<ReplayUnit, Vec<PathBuf>>,
}

impl Ungated {
    /// Records `unit`'s fresh objects, replacing an earlier compile's.
    fn compiled(&mut self, unit: ReplayUnit, objects: Vec<PathBuf>) {
        self.objects.insert(unit, objects);
    }

    /// Whether `unit` compiled since the last accepted patch.
    #[cfg(test)]
    fn contains(&self, unit: &ReplayUnit) -> bool {
        self.objects.contains_key(unit)
    }

    /// Every ungated object, in unit order.
    #[cfg(test)]
    fn inputs(&self) -> Vec<PathBuf> {
        self.objects().map(|(_, object)| object.clone()).collect()
    }

    /// Every ungated object with its unit, in unit order: the layout gate's
    /// input, through [`gated_inputs`].
    fn objects(&self) -> impl Iterator<Item = (&ReplayUnit, &PathBuf)> {
        self.objects
            .iter()
            .flat_map(|(unit, objects)| objects.iter().map(move |object| (unit, object)))
    }

    /// Every ungated object passed the gates in an accepted candidate.
    fn accept(&mut self) {
        self.objects.clear();
    }
}

/// How the session reaches the app.
pub(super) enum AppLink {
    /// Hot patching is off for this process; every change restarts.
    RestartOnly { reason: String },
    Live {
        client: DevtoolsClient,
        endpoint: SocketAddr,
        pid: u32,
        anchor_runtime: u64,
        /// Whether the app runs on this host and so can read a file the
        /// builder wrote. `false` for a device reached through `adb
        /// forward`: the endpoint is loopback on this host, but the app is
        /// another machine, so a patch is always uploaded in chunks there,
        /// whatever its `patch_file_hand_off` says.
        same_host: bool,
    },
}

/// Connects to the app's devtools endpoint and decides whether this
/// session can patch it. Nothing is sent to a non-loopback endpoint (an
/// IPv4-mapped IPv6 address counts as non-loopback); an app without
/// `HotPatch` gives the precondition its `hotpatch_info` refusal names.
fn attach_app(endpoint: SocketAddr, token: Option<&str>, triple: &str) -> AppLink {
    attach(endpoint, token, triple, true)
}

/// [`attach_app`] for an app on a device, reached through a host loopback
/// port that forwards to it (`adb forward`): the session never hands such
/// an app a host path, and uploads every patch in chunks.
pub(super) fn attach_device_app(
    endpoint: SocketAddr,
    token: Option<&str>,
    triple: &str,
) -> AppLink {
    attach(endpoint, token, triple, false)
}

fn attach(endpoint: SocketAddr, token: Option<&str>, triple: &str, same_host: bool) -> AppLink {
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
        same_host,
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
    /// The stages the current attempt's session side has timed so far.
    timings: PatchTimings,
    /// Whether the current attempt reached the compile.
    compiled: bool,
    /// The last attempt that reached the compile ([`Self::last_timings`]).
    last_timings: Option<PatchTimings>,
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

    /// The stage timings of the last change that reached the compile, as
    /// appended to [`PATCH_TIMINGS_FILE`]; `None` before the first.
    pub fn last_timings(&self) -> Option<&PatchTimings> {
        self.last_timings.as_ref()
    }

    /// Patches the running app with `paths`' changes, or says why it must
    /// be relaunched. `paths` are the changed files as the watcher reports
    /// them.
    pub fn on_change(&mut self, paths: &[PathBuf]) -> Outcome {
        if let Some(reason) = &self.restart {
            return Outcome::RestartRequired(reason.clone());
        }
        let started = Instant::now();
        self.timings = PatchTimings::default();
        let outcome = self.change(paths, started);
        self.record_timings(&outcome, started);
        if let Outcome::RestartRequired(reason) = &outcome {
            self.restart = Some(reason.clone());
        }
        outcome
    }

    /// Completes the attempt's [`PatchTimings`] with the builder's stages
    /// and, when the compile ran, keeps it and appends its line to
    /// [`PATCH_TIMINGS_FILE`] (best effort: a write failure never changes the
    /// outcome).
    fn record_timings(&mut self, outcome: &Outcome, started: Instant) {
        let timings = PatchTimings {
            total: started.elapsed(),
            ..self.timings
        }
        .with_builder(self.builder.take_timings());
        if !std::mem::take(&mut self.compiled) {
            return;
        }
        self.last_timings = Some(timings);
        let line = format!(
            "hot patch timings: outcome={} {timings}\n",
            outcome_word(outcome)
        );
        let path = self.accepted.dir().join(PATCH_TIMINGS_FILE);
        let appended = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes()));
        drop(appended);
    }

    fn change(&mut self, paths: &[PathBuf], started: Instant) -> Outcome {
        let restart = Outcome::RestartRequired;
        let (client, pid, anchor_runtime, same_host) = match &self.app {
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
                same_host,
            } => {
                let peer_loopback = client.peer_addr().is_some_and(|a| a.ip().is_loopback());
                if !endpoint.ip().is_loopback() || !peer_loopback {
                    return restart(RestartReason::EndpointNotLoopback {
                        endpoint: *endpoint,
                    });
                }
                (client, *pid, *anchor_runtime, *same_host)
            }
        };

        let units = match classify_paths(self.builder.as_ref(), paths) {
            Ok(units) => units,
            Err(reason) => return restart(reason),
        };
        if units.is_empty() {
            return Outcome::NoChange;
        }
        self.compiled = true;
        let (layouts, seams) = match self.builder.compile(&units) {
            Ok(Compiled::Nothing) => return Outcome::NoChange,
            Ok(Compiled::Failed { diagnostics }) => return Outcome::CompileFailed { diagnostics },
            Ok(Compiled::Candidate { layouts, seams }) => (layouts, seams),
            Err(err) => return restart(RestartReason::builder(&err)),
        };
        let checking = Instant::now();
        let present = match self.accepted.check(&layouts, seams) {
            Ok(present) => present,
            Err(reason) => return restart(reason),
        };

        let info = client.hotpatch_info();
        self.timings.check = checking.elapsed();
        let info = match info {
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
        // A PE candidate's layout table exists only once its patch is
        // linked: gate it now, before anything is sent.
        let layouts = match self.builder.linked_layouts() {
            Some(linked_layouts) => match self.accepted.check(&linked_layouts, present.clone()) {
                Ok(_) => linked_layouts,
                Err(reason) => return restart(reason),
            },
            None => layouts,
        };
        let len = linked.bytes.len() as u64;
        self.timings.patch_bytes = len;
        self.timings.table_entries = linked.table.map.len() as u64;
        if let Some(reason) = self
            .budget
            .exceeded(n, info.patch_bytes_loaded.saturating_add(len))
        {
            return restart(reason);
        }

        let patch_id = self.next_patch_id;
        self.next_patch_id += 1;
        // The endpoint and peer were checked loopback above, so an app on
        // this host that takes the hand-off reads the file this host just
        // wrote. A device app behind `adb forward` is never named a host
        // path, whatever it advertises.
        let file = if info.patch_file_hand_off && same_host {
            match hand_off(&linked.path, &linked.bytes) {
                Ok(file) => file,
                Err(err) => return restart(RestartReason::builder(&err)),
            }
        } else {
            None
        };
        let uploading = Instant::now();
        if file.is_none()
            && let Err(err) = client.upload_patch(patch_id, &linked.bytes)
        {
            return restart(RestartReason::DevtoolsFailed {
                detail: format!("{err:#}"),
            });
        }
        self.timings.upload_patch = uploading.elapsed();
        // The jump table travels as chunks on every path, the hand-off too:
        // a large app's table is far past the request line cap.
        let uploading = Instant::now();
        let table_len = client.upload_table(patch_id, &linked.table);
        self.timings.upload_table = uploading.elapsed();
        let table_len = match table_len {
            Ok(table_len) => table_len,
            Err(err) => {
                return restart(RestartReason::DevtoolsFailed {
                    detail: format!("{err:#}"),
                });
            }
        };
        let params = ApplyPatchParams {
            patch_id,
            len,
            pid,
            anchor_runtime,
            table: linked.table,
            table_len,
            expected_seams: u32::try_from(present.len()).unwrap_or(u32::MAX),
            file,
        };
        let applying = Instant::now();
        let outcome = client.apply_patch(&params);
        self.timings.apply = applying.elapsed();
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(err) => {
                let detail = format!("{err:#}");
                return restart(if err.downcast_ref::<DevtoolsRpcError>().is_some() {
                    RestartReason::PatchRefused { detail }
                } else if err.downcast_ref::<RequestTooLarge>().is_some() {
                    // Refused before sending: nothing reached the app.
                    RestartReason::DevtoolsFailed { detail }
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
        self.builder.accepted();
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

    // The fat build compiles for the host, so the graph is the host's: a
    // dependency gated to another platform is no edge of it.
    let rustc_version = capture::rustc_version(runner)?;
    let triple = host_triple(&rustc_version)?;
    let metadata =
        super::graph::cargo_metadata(runner, &start.root.join("Cargo.toml"), Some(&triple))?;
    let mut graph = WorkspaceGraph::from_metadata(&metadata, start.package, start.bin)?;
    check_debuginfo(host.env, start.root, graph.workspace_root())?;
    let target_dir = target_directory(&metadata)?;

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
    // Local non-members (a `--frust-path` checkout) are captured too, so an
    // edit to one replays with its dependents instead of restarting.
    let non_members = graph.non_members();
    let scope_dir = prepare_scope_dir_for(&target_dir, &scope, &non_members)?;
    let members: Vec<String> = graph
        .packages()
        .iter()
        .filter(|p| p.member && p.name != start.package)
        .map(|p| p.name.clone())
        .chain(non_members.names())
        .collect();
    capture::bust_fingerprints(
        &capture::fingerprint_dir(&target_dir, None, "dev"),
        &scope_dir,
        start.package,
        &members,
    )?;

    let fat_dir = hotpatch_root(&target_dir)
        .join(FAT_DIR)
        .join(scope.dir_name_for(&non_members)?);
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
        WrapperSetup {
            frust_exe: &host.frust_exe,
            scope_dir: &scope_dir,
            ambient_names: &capture::ambient_env_names(),
        },
        &link,
    );
    run_fat_build(runner, &fat, start.root, on_line)?;

    let link_args = read_link_args(&link.args_file)?;
    if !non_members.is_empty() {
        graph.replay_non_members(&load_records(&scope_dir)?);
        // An empty set means the link line was not read the way this
        // expects (a response file, a wrapped archive): keep every local
        // package in the image rather than drop one silently.
        let linked = linked_crates(&link_args);
        if !linked.is_empty() {
            graph.set_linked_crates(linked);
        }
    }
    let base = link_base(
        host,
        BaseRequest {
            graph,
            link_args,
            image_unit: tip_bin.clone(),
            image: fat_dir.join(desktop_image_name(flavor, &tip_bin.target)),
            custom_linker: custom_linker(host.env, &triple),
            upload_strip: None,
            target,
            flavor,
            target_dir,
            archive_dir: &fat_dir,
            scope_dir,
            session: desktop_session_name(&tip_bin.target),
        },
    )?;

    let plan = desktop_run::desktop_exe_plan(start.root, base.image(), start.info);
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

    Ok((open_session(base, app, budget), child))
}

/// The desktop session directory's name, `session-<bin>`: one running
/// desktop app per bin on this host. The Android name also carries the
/// triple and the device ([`super::android::session_name`]).
pub(super) fn desktop_session_name(bin: &str) -> String {
    format!("session-{bin}")
}

/// The fat image's file name: the bin's name, `<bin>.exe` on Msvc (whose
/// PDB the linker then writes beside it as `<bin>.pdb`).
fn desktop_image_name(flavor: LinkerFlavor, bin: &str) -> String {
    match flavor {
        LinkerFlavor::Msvc => format!("{bin}.exe"),
        LinkerFlavor::Darwin | LinkerFlavor::Gnu => bin.to_string(),
    }
}

/// What [`link_base`] links the base image from: the fat build's graph,
/// captured link line and capture scope, and where the image goes. Shared
/// by [`start_desktop`] and the Android start
/// ([`super::android::start_android`]).
pub(super) struct BaseRequest<'a> {
    pub graph: WorkspaceGraph,
    /// The image link's captured arguments (expanded).
    pub link_args: Vec<String>,
    /// The unit whose link is the running image: the tip bin on desktop,
    /// the tip lib (its `cdylib`) on Android.
    pub image_unit: ReplayUnit,
    /// Where the fat image is written, under the cargo target dir.
    pub image: PathBuf,
    /// The build's configured linker, if any (see
    /// [`fat_link::linker_program`]).
    pub custom_linker: Option<PathBuf>,
    /// The tool that strips each patch into the copy the app is sent (the
    /// NDK's `llvm-strip` on Android); `None` sends the linked patch.
    pub upload_strip: Option<PathBuf>,
    pub target: Target,
    pub flavor: LinkerFlavor,
    pub target_dir: PathBuf,
    /// Where `libdeps-<hash>.a` lives: one directory per capture scope.
    pub archive_dir: &'a Path,
    pub scope_dir: PathBuf,
    /// The session directory's name under `<target>/frust-hotpatch`.
    pub session: String,
}

/// A linked base image and the real builder over it, ready for a session
/// once the app is running.
pub(super) struct FatBase {
    builder: DesktopBuilder,
    accepted: AcceptedSets,
    image: PathBuf,
}

impl FatBase {
    /// The fat image, under the cargo target dir.
    pub(super) fn image(&self) -> &Path {
        &self.image
    }

    /// The file the session's symbol cache was read from: the unstripped
    /// fat image itself.
    pub(super) fn symbol_source(&self) -> &Path {
        self.builder.cache.path()
    }
}

/// The fat link and base-image half of a session start, shared by every
/// target: reads the capture records, links the fat image with the image
/// unit's captured environment, extracts the base layout table and seam
/// instances, starts the session's accepted sets, reads the symbol cache
/// from the image just linked and seeds the graph's dep-info.
pub(super) fn link_base(
    host: &SessionHost<'_>,
    request: BaseRequest<'_>,
) -> Result<FatBase, HotpatchError> {
    base(host, request, true)
}

/// [`link_base`] over an image another tool already linked from the fat
/// build (Xcode's `Runner` on the iOS simulator, [`super::ios_sim`]): no
/// fat link, and the symbol cache is read from `request.image` as it is.
pub(super) fn adopt_base(
    host: &SessionHost<'_>,
    request: BaseRequest<'_>,
) -> Result<FatBase, HotpatchError> {
    base(host, request, false)
}

fn base(
    host: &SessionHost<'_>,
    request: BaseRequest<'_>,
    relink: bool,
) -> Result<FatBase, HotpatchError> {
    let runner: &dyn ProcessRunner = &*host.runner;
    let BaseRequest {
        mut graph,
        link_args,
        image_unit,
        image,
        custom_linker,
        upload_strip,
        target,
        flavor,
        target_dir,
        archive_dir,
        scope_dir,
        session,
    } = request;
    let records = load_records(&scope_dir)?;
    let tip_record = records.get(&image_unit.record_key()).ok_or_else(|| {
        HotpatchError::unsupported(format!(
            "the fat build captured no `{}` invocation",
            image_unit.record_key()
        ))
    })?;
    let tip_env = replay_env(tip_record);
    let linker = fat_link::flavor_linker_program(flavor, custom_linker.as_deref(), || {
        let libdir = host_target_libdir(runner, tip_record, &graph.replay_cwd(&image_unit))?;
        fat_link::bundled_lld(&libdir)
    })?;
    let image = if relink {
        fat_link::fat_link(
            runner,
            &FatLinkRequest {
                flavor,
                linker: &linker,
                link_args: &link_args,
                envs: &tip_env,
                target_dir: &target_dir,
                archive_dir,
                exe: &image,
            },
        )?
        .exe
    } else {
        image
    };

    let rlibs = member_rlibs(&link_args, &graph);
    let tip_objects = tip_objects(&link_args);
    let crates = replayable_crates(&graph);
    let base_layouts = if flavor == LinkerFlavor::Msvc {
        // CodeView type records are readable from a PDB only: the fat
        // exe's, which `/WHOLEARCHIVE` made cover every member object.
        pdb_layout::extract(std::slice::from_ref(&image), &crates)?.table
    } else {
        let typed = typed_objects(&tip_objects, &crates)?;
        layout::extract_with(&gated_inputs(&rlibs, &graph)?, typed.reads, &crates)?.table
    };
    // Every base tip object's types are in the base table: a replay that
    // leaves one byte-identical need not read it again.
    let accepted_objects = if flavor == LinkerFlavor::Msvc {
        HashSet::new()
    } else {
        object_digests(&tip_objects)?.into_iter().collect()
    };
    let base_seams = seam_set(
        flavor,
        &rlibs
            .values()
            .cloned()
            .chain(tip_objects)
            .collect::<Vec<_>>(),
    )?;
    let accepted = AcceptedSets::begin(&target_dir, &session, base_layouts, base_seams)?;
    let cache = SymbolCache::load(&image, target)?;
    seed_dep_info(&mut graph, &records);

    let builder = DesktopBuilder {
        runner: Arc::clone(&host.runner),
        graph,
        records,
        modified: super::graph::ModifiedSet::new(),
        dirty: BTreeSet::new(),
        ungated: Ungated::default(),
        rlibs,
        tip_link_args: link_args,
        tip_env,
        image_unit,
        cache,
        target,
        flavor,
        linker,
        upload_strip,
        target_dir,
        session,
        session_dir: accepted.dir().to_path_buf(),
        scope_dir,
        frust_exe: host.frust_exe.clone(),
        crates,
        tip_replays: 0,
        linked_layouts: None,
        timings: PatchTimings::default(),
        accepted_objects,
        tip_digests: Vec::new(),
        image_reads: Vec::new(),
        seam_names: HashMap::new(),
    };
    Ok(FatBase {
        builder,
        accepted,
        image,
    })
}

/// The session over `base`'s builder and the running app `app`.
pub(super) fn open_session(base: FatBase, app: AppLink, budget: Budget) -> HotSession {
    let FatBase {
        builder, accepted, ..
    } = base;
    let images = vec![builder.cache.symbols().clone()];
    HotSession {
        builder: Box::new(builder),
        app,
        accepted,
        images,
        budget,
        restart: None,
        next_patch_id: 1,
        timings: PatchTimings::default(),
        compiled: false,
        last_timings: None,
    }
}

/// What the fat image's output announced.
pub(super) enum Announced {
    Endpoint(Discovery),
    /// The devtools service did not start, or said nothing in time.
    Failure(String),
    Exited,
}

/// A raised cancel flag ended [`read_discovery_cancellable`]'s wait.
pub(super) struct Cancelled;

/// Reads the child's output up to its devtools discovery line, forwarding
/// every line before it (token redacted) to `on_line`. Nothing cancels
/// this wait (the desktop start has no cancel seam).
pub(super) fn read_discovery(child: &mut StreamHandle, on_line: &mut dyn FnMut(&str)) -> Announced {
    match wait_for_discovery(child, on_line, || None::<Infallible>) {
        Ok(announced) => announced,
        Err(never) => match never {},
    }
}

/// [`read_discovery`] that also observes `cancel` at every poll: a raised
/// flag ends the wait with [`Cancelled`], and the child is the caller's to
/// stop.
pub(super) fn read_discovery_cancellable(
    child: &mut StreamHandle,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Announced, Cancelled> {
    wait_for_discovery(child, on_line, || {
        cancel.load(Ordering::SeqCst).then_some(Cancelled)
    })
}

/// The discovery wait both readers share; `cancelled` is polled before
/// every read and its `Some` ends the wait.
fn wait_for_discovery<C>(
    child: &mut StreamHandle,
    on_line: &mut dyn FnMut(&str),
    mut cancelled: impl FnMut() -> Option<C>,
) -> Result<Announced, C> {
    let deadline = Instant::now() + DISCOVERY_DEADLINE;
    loop {
        if let Some(cancelled) = cancelled() {
            return Err(cancelled);
        }
        match child.lines.try_recv() {
            Ok(line) => {
                on_line(&redact_discovery_token(&line));
                if let Some(discovery) = parse_discovery_line(&line) {
                    return Ok(Announced::Endpoint(discovery));
                }
                if let Some(reason) = parse_failure_line(&line) {
                    return Ok(Announced::Failure(format!(
                        "the devtools service did not start: {reason}"
                    )));
                }
            }
            Err(TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(DISCOVERY_POLL);
            }
            Err(TryRecvError::Empty) => {
                return Ok(Announced::Failure(format!(
                    "the app announced no devtools endpoint within {}s",
                    DISCOVERY_DEADLINE.as_secs()
                )));
            }
            Err(TryRecvError::Disconnected) => return Ok(Announced::Exited),
        }
    }
}

/// Runs the fat build, forwarding cargo's rendered diagnostics and any
/// non-JSON line to `on_line`. A failed build is
/// [`StartError::FatBuildFailed`] with its error diagnostics.
pub(super) fn run_fat_build(
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
pub(super) fn target_directory(metadata: &str) -> Result<PathBuf, HotpatchError> {
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

/// `rustc --print target-libdir` for the host, run as `record`'s rustc
/// with its recorded environment in `cwd` (so a rustup proxy picks the
/// toolchain the fat build used): `<sysroot>/lib/rustlib/<host>/lib`,
/// beside the bundled `rust-lld`'s `bin`.
fn host_target_libdir(
    runner: &dyn ProcessRunner,
    record: &RustcRecord,
    cwd: &Path,
) -> Result<PathBuf, HotpatchError> {
    let rustc = record
        .rustc()
        .ok_or_else(|| HotpatchError::unsupported("the image capture has no rustc"))?;
    let env = replay_env(record);
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let output = runner
        .run_streaming(
            rustc,
            &["--print", "target-libdir"],
            Some(cwd),
            &env,
            &mut |_| {},
        )
        .map_err(|err| HotpatchError::Process {
            detail: format!("failed to spawn `{rustc} --print target-libdir`: {err:#}"),
        })?;
    let libdir = output.stdout.trim();
    if !output.success || libdir.is_empty() {
        return Err(HotpatchError::unsupported(format!(
            "`{rustc} --print target-libdir` failed: {}",
            output.stderr.trim()
        )));
    }
    Ok(PathBuf::from(libdir))
}

/// The rustflags cargo applies: `CARGO_ENCODED_RUSTFLAGS`, else `RUSTFLAGS`.
pub(super) fn rustflags(env: &dyn EnvLookup) -> Vec<String> {
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
pub(super) fn remove_stale(path: &Path) -> Result<(), HotpatchError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(HotpatchError::io(
            format!("removing `{}`", path.display()),
            err,
        )),
    }
}

/// The seam instances `inputs` (objects and rlibs) define. Darwin and Gnu:
/// [`SeamSet::from_inputs`]. Msvc: the same rule over the defined symbol
/// names of COFF objects, read here, since the shared object reader takes
/// only the DWARF gates' Mach-O and ELF.
fn seam_set(flavor: LinkerFlavor, inputs: &[PathBuf]) -> Result<SeamSet, HotpatchError> {
    if flavor != LinkerFlavor::Msvc {
        return SeamSet::from_inputs(inputs);
    }
    let mut names = Vec::new();
    for input in inputs {
        coff_definitions(input, &mut names)?;
    }
    SeamSet::from_symbols(names.iter().map(String::as_str))
}

/// Appends the defined symbol names of the COFF object at `path`, or of
/// every `.o` member of the archive there. An input holding no object, or
/// an object in another format, is [`HotpatchError::BuilderUnsupported`].
fn coff_definitions(path: &Path, names: &mut Vec<String>) -> Result<(), HotpatchError> {
    use object::read::archive::ArchiveFile;
    use object::{Object as _, ObjectSymbol as _};

    let data = std::fs::read(path)
        .map_err(|err| HotpatchError::io(format!("reading {}", path.display()), err))?;
    let mut read = |label: &str, bytes: &[u8]| {
        let file = object::File::parse(bytes).map_err(|err| {
            HotpatchError::unsupported(format!("{label} is not an object file: {err}"))
        })?;
        if file.format() != object::BinaryFormat::Coff {
            return Err(HotpatchError::unsupported(format!(
                "{label} is {:?}, not a COFF object",
                file.format()
            )));
        }
        names.extend(
            file.symbols()
                .filter(|symbol| symbol.is_definition())
                .filter_map(|symbol| symbol.name().ok().map(str::to_string)),
        );
        Ok(())
    };
    let mut count = 0usize;
    if data.starts_with(b"!<arch>\n") {
        let archive = ArchiveFile::parse(&*data).map_err(|err| {
            HotpatchError::unsupported(format!(
                "{} is not a readable archive: {err}",
                path.display()
            ))
        })?;
        for member in archive.members() {
            let member = member.map_err(|err| {
                HotpatchError::unsupported(format!("{}: bad archive member: {err}", path.display()))
            })?;
            let name = String::from_utf8_lossy(member.name()).into_owned();
            if !name.ends_with(".o") {
                continue;
            }
            let label = format!("{}({name})", path.display());
            let bytes = member.data(&*data).map_err(|err| {
                HotpatchError::unsupported(format!("{label}: unreadable member: {err}"))
            })?;
            read(&label, bytes)?;
            count += 1;
        }
    } else {
        read(&path.display().to_string(), &data)?;
        count += 1;
    }
    if count == 0 {
        return Err(HotpatchError::unsupported(format!(
            "{} holds no object file",
            path.display()
        )));
    }
    Ok(())
}

/// The `.rcgu.o` objects a captured tip link names: the image unit's own
/// code.
fn tip_objects(link_args: &[String]) -> Vec<PathBuf> {
    link_args
        .iter()
        .filter(|arg| arg.ends_with(".rcgu.o"))
        .map(PathBuf::from)
        .collect()
}

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
///
/// Each object's DWARF is read once, all of them in parallel
/// ([`layout::read_inputs`]), and the reads come back with the paths so the
/// layout table is built from them ([`layout::extract_with`]) rather than
/// from a second read.
fn typed_objects(objects: &[PathBuf], crates: &[String]) -> Result<TypedObjects, HotpatchError> {
    typed_objects_among(objects, crates, true)
}

/// [`typed_objects`], with the "at least one object carries DWARF" rule
/// applied only when `require_dwarf`: a round whose other tip objects were
/// skipped as already accepted ([`DesktopBuilder::accepted_objects`]) may
/// recompile only objects without any.
fn typed_objects_among(
    objects: &[PathBuf],
    crates: &[String],
    require_dwarf: bool,
) -> Result<TypedObjects, HotpatchError> {
    let mut typed = TypedObjects::default();
    let mut with_dwarf = 0usize;
    for (object, read) in objects.iter().zip(layout::read_inputs(objects, crates)) {
        match read {
            Ok(read) if read.type_dies() > 0 => {
                typed.paths.push(object.clone());
                typed.reads.push(read);
                with_dwarf += 1;
            }
            // DWARF without a type: nothing to compare.
            Ok(_) => with_dwarf += 1,
            Err(HotpatchError::BuilderUnsupported { detail }) if detail.contains(NO_DWARF) => {}
            Err(err) => return Err(err),
        }
    }
    if require_dwarf && !objects.is_empty() && with_dwarf == 0 {
        return Err(HotpatchError::unsupported(format!(
            "none of the tip's {} objects carries DWARF; the layout gate needs `debug = true`",
            objects.len()
        )));
    }
    Ok(typed)
}

/// [`typed_objects`]' answer: the typed objects and their DWARF reads, in
/// the same order.
#[derive(Debug, Default)]
struct TypedObjects {
    paths: Vec<PathBuf>,
    reads: Vec<layout::InputTypes>,
}

/// The SHA-256 of an object's or rlib's bytes: what the gates key their
/// per-object reuse on ([`DesktopBuilder::accepted_objects`],
/// [`DesktopBuilder::seam_names`]), so a file is reused only when its
/// content is exactly what was read before, whatever its path or mtime.
type ObjectDigest = [u8; 32];

/// [`ObjectDigest`] of each of `paths`, in order (in parallel).
fn object_digests(paths: &[PathBuf]) -> Result<Vec<ObjectDigest>, HotpatchError> {
    layout::parallel_map(paths, |path| {
        use sha2::Digest as _;
        let bytes = std::fs::read(path)
            .map_err(|err| HotpatchError::io(format!("reading {}", path.display()), err))?;
        Ok(sha2::Sha256::digest(&bytes).into())
    })
    .into_iter()
    .collect()
}

/// The defined symbols of `input` (an object, or an rlib's `.o` members)
/// that [`SeamSet::from_symbols`] does not skip: those whose demangled form
/// is a seam, or a shape the seam parser refuses. Feeding these, in input
/// order, to `from_symbols` answers exactly what [`SeamSet::from_inputs`]
/// answers over every symbol, its refusals included: the symbols left out
/// are the ones it ignores.
fn seam_candidates(input: &Path) -> Result<Vec<String>, HotpatchError> {
    let mut names = Vec::new();
    layout::for_each_object(input, |_, file| {
        use object::{Object as _, ObjectSymbol as _};
        for symbol in file.symbols().filter(|symbol| symbol.is_definition()) {
            let Ok(name) = symbol.name() else { continue };
            let Ok(demangled) = rustc_demangle::try_demangle(name) else {
                continue;
            };
            if !matches!(seams::parse_seam(&format!("{demangled:#}")), Ok(None)) {
                names.push(name.to_string());
            }
        }
        Ok(())
    })?;
    Ok(names)
}

/// The rlib a captured link names for each lib unit in the image (members'
/// and replayable non-members'): `lib<crate>.rlib` or
/// `lib<crate>-<hash>.rlib`.
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

/// The crate names (rustc spelling) of every rlib a captured link line
/// names, `lib<crate>.rlib` or `lib<crate>-<hash>.rlib`: what the image
/// links ([`WorkspaceGraph::set_linked_crates`]).
fn linked_crates(link_args: &[String]) -> BTreeSet<String> {
    link_args
        .iter()
        .filter_map(|arg| {
            let name = Path::new(arg).file_name()?.to_str()?;
            let stem = name.strip_prefix("lib")?.strip_suffix(".rlib")?;
            Some(
                stem.split_once('-')
                    .map_or(stem, |(name, _)| name)
                    .to_string(),
            )
        })
        .collect()
}

/// The one input rule of the DWARF layout gate, for the base table (each
/// lib unit's rlib in the image) and every candidate table (each ungated
/// object): a member unit's input is always read, so an empty one still
/// fails closed; a replayable non-member's is read only when it holds code
/// or data ([`layout::holds_code_or_data`]). A platform shell compiled
/// empty for this host has no DWARF to read and no value to protect, and
/// leaving it out of one table but not the other would refuse every cascade
/// that replays it.
fn gated_inputs<'a>(
    inputs: impl IntoIterator<Item = (&'a ReplayUnit, &'a PathBuf)>,
    graph: &WorkspaceGraph,
) -> Result<Vec<PathBuf>, HotpatchError> {
    let mut gated = Vec::new();
    for (unit, input) in inputs {
        let member = graph
            .packages()
            .iter()
            .any(|package| package.member && package.name == unit.package);
        if member || layout::holds_code_or_data(input)? {
            gated.push(input.clone());
        }
    }
    Ok(gated)
}

/// Every replay unit's crate name (members' targets and replayable
/// non-members' libs): the crates whose types the layout gate fingerprints.
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
/// against the fat image. Named for the desktop, where the image unit is the
/// tip bin; the Android start ([`super::android`]) runs the same builder with
/// the tip lib's `cdylib` as the image unit.
struct DesktopBuilder {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    graph: WorkspaceGraph,
    records: BTreeMap<RecordKey, RustcRecord>,
    modified: super::graph::ModifiedSet,
    /// Units recorded as changed whose replay has not yet succeeded; they
    /// are replayed again with the next change, so a patch never links a
    /// stale object after a failed compile.
    dirty: BTreeSet<ReplayUnit>,
    /// Units compiled since the last accepted patch, with their objects:
    /// the layout gate's input for every candidate until one is accepted.
    ungated: Ungated,
    /// The current rlib of each lib unit in the image.
    rlibs: BTreeMap<ReplayUnit, PathBuf>,
    /// The tip's latest captured link: the fat build's until the image unit
    /// is replayed.
    tip_link_args: Vec<String>,
    tip_env: Vec<(String, String)>,
    /// The unit whose link is the running image, replayed with its link
    /// intercepted: the tip bin on desktop, the tip lib on Android (whose
    /// code then enters a patch as objects, never as its rlib).
    image_unit: ReplayUnit,
    cache: SymbolCache,
    target: Target,
    flavor: LinkerFlavor,
    linker: String,
    /// See [`BaseRequest::upload_strip`].
    upload_strip: Option<PathBuf>,
    target_dir: PathBuf,
    session: String,
    session_dir: PathBuf,
    scope_dir: PathBuf,
    frust_exe: PathBuf,
    crates: Vec<String>,
    tip_replays: u32,
    /// Msvc: the layout table of the patch [`PatchBuilder::link`] just
    /// linked, read from its PDB, until the session takes it.
    linked_layouts: Option<LayoutTable>,
    /// The stages timed since the session last took them.
    timings: PatchTimings,
    /// Digests of image-unit objects whose types the accepted set already
    /// holds: the base's tip objects, and every tip object of each accepted
    /// patch. A replayed tip object with one of these digests is the very
    /// bytes that were gated and accepted (an incremental rebuild leaves
    /// every untouched codegen unit so), so it adds nothing to the candidate
    /// and its DWARF is not read again. Dropping it from the candidate is
    /// what not replaying it would do: the gate still reads every object
    /// whose bytes are new.
    accepted_objects: HashSet<ObjectDigest>,
    /// The current tip objects' digests, joined to `accepted_objects` once
    /// a candidate built on them is accepted.
    tip_digests: Vec<ObjectDigest>,
    /// This round's DWARF reads of the image unit's typed objects, taken by
    /// the layout table it builds so no object is read twice.
    image_reads: Vec<(PathBuf, layout::InputTypes)>,
    /// [`seam_candidates`] per input digest: a patch's inputs are mostly the
    /// unchanged objects of the previous one.
    seam_names: HashMap<ObjectDigest, Vec<String>>,
}

impl DesktopBuilder {
    /// Every modified lib's rlib, dependents before their dependencies (the
    /// order a static link resolves archives in). A lib that is the image
    /// unit itself (Android's tip lib) is left out: its objects are on the
    /// image link line already.
    fn modified_rlibs(&self) -> Result<Vec<PathBuf>, HotpatchError> {
        let order = self.graph.replay_order(self.modified.units())?;
        order
            .iter()
            .rev()
            .filter(|unit| unit.kind == TargetKind::Lib && **unit != self.image_unit)
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

    /// Replays the image unit with its link step intercepted and returns the
    /// intercepted link's arguments, which name the fresh objects the patch
    /// links. The caller installs them as `tip_link_args` only once those
    /// objects are in the ungated ledger, so a failure in between leaves
    /// the previous link line in place. `Ok(Err(diagnostics))` for a
    /// compile error.
    fn replay_image_unit(&mut self) -> Result<Result<Vec<String>, Vec<String>>, HotpatchError> {
        let key = self.image_unit.record_key();
        let record = self.records.get(&key).ok_or_else(|| {
            HotpatchError::unsupported(format!("no captured rustc invocation `{key}`"))
        })?;
        let rustc = record
            .rustc()
            .ok_or_else(|| HotpatchError::unsupported(format!("the `{key}` capture has no rustc")))?
            .to_string();
        let mut args = replay_args_image(record)?;
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
        let cwd = self.graph.replay_cwd(&self.image_unit);
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let env_refs: Vec<(&str, &str)> =
            env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let output = self
            .runner
            .run_streaming_scrubbed(&rustc, &argv, Some(&cwd), &env_refs, &removed, &mut |_| {})
            .map_err(|err| HotpatchError::Process {
                detail: format!(
                    "failed to spawn `{rustc}` to replay {}: {err:#}",
                    self.image_unit
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
        Ok(Ok(read_link_args(&link.args_file)?))
    }
}

impl PatchBuilder for DesktopBuilder {
    fn classify(&self, path: &Path) -> PathClass {
        let class = self.graph.classify(path);
        if self.image_unit.kind == TargetKind::Bin {
            return class;
        }
        // A lib image (Android's cdylib) contains no bin: a file only a bin
        // compiles is nothing the running image was built from.
        match class {
            PathClass::Replayable { units } => {
                let units: BTreeSet<ReplayUnit> = units
                    .into_iter()
                    .filter(|unit| unit.kind == TargetKind::Lib)
                    .collect();
                if units.is_empty() {
                    PathClass::Unaffected
                } else {
                    PathClass::Replayable { units }
                }
            }
            other => other,
        }
    }

    fn compile(&mut self, units: &BTreeSet<ReplayUnit>) -> Result<Compiled, HotpatchError> {
        let compiling = Instant::now();
        let compiled = self.compile_timed(units, compiling);
        if self.timings.compile.is_zero() {
            self.timings.compile = compiling.elapsed();
        }
        compiled
    }

    fn link(&mut self, n: u32, anchor_runtime: u64) -> Result<LinkedPatch, HotpatchError> {
        self.link_timed(n, anchor_runtime)
    }

    fn linked_layouts(&mut self) -> Option<LayoutTable> {
        self.linked_layouts.take()
    }

    fn accepted(&mut self) {
        self.ungated.accept();
        self.accepted_objects
            .extend(self.tip_digests.iter().copied());
    }

    fn take_timings(&mut self) -> PatchTimings {
        std::mem::take(&mut self.timings)
    }
}

impl DesktopBuilder {
    /// The typed objects among the image unit's fresh `objects`, read once:
    /// an object whose bytes are already accepted
    /// ([`Self::accepted_objects`]) is left out, the others are read and
    /// their reads kept for this round's layout table
    /// ([`Self::image_reads`]). Records the objects' digests for
    /// [`PatchBuilder::accepted`].
    fn fresh_typed_objects(&mut self, objects: &[PathBuf]) -> Result<Vec<PathBuf>, HotpatchError> {
        let digests = object_digests(objects)?;
        let fresh: Vec<PathBuf> = objects
            .iter()
            .zip(&digests)
            .filter(|(_, digest)| !self.accepted_objects.contains(*digest))
            .map(|(object, _)| object.clone())
            .collect();
        let skipped = objects.len() - fresh.len();
        let typed = typed_objects_among(&fresh, &self.crates, skipped == 0)?;
        self.tip_digests = digests;
        self.image_reads = typed.paths.iter().cloned().zip(typed.reads).collect();
        Ok(typed.paths)
    }

    /// [`seam_set`] of [`Self::patch_inputs`], each input's seam candidates
    /// read once per content ([`Self::seam_names`]). Msvc reads COFF
    /// symbols every time.
    fn seam_set(&mut self) -> Result<SeamSet, HotpatchError> {
        let inputs = self.patch_inputs()?;
        if self.flavor == LinkerFlavor::Msvc {
            return seam_set(self.flavor, &inputs);
        }
        let digests = object_digests(&inputs)?;
        let missing: Vec<(PathBuf, ObjectDigest)> = inputs
            .iter()
            .zip(&digests)
            .filter(|(_, digest)| !self.seam_names.contains_key(*digest))
            .map(|(input, digest)| (input.clone(), *digest))
            .collect();
        let read = layout::parallel_map(&missing, |(input, _)| seam_candidates(input));
        for ((_, digest), names) in missing.iter().zip(read) {
            self.seam_names.insert(*digest, names?);
        }
        SeamSet::from_symbols(
            digests
                .iter()
                .filter_map(|digest| self.seam_names.get(digest))
                .flatten()
                .map(String::as_str),
        )
    }

    /// [`PatchBuilder::compile`], recording `compile` (the replays, from
    /// `compiling`) and `gate` (the candidate's DWARF and seam reads) as it
    /// reaches each.
    fn compile_timed(
        &mut self,
        units: &BTreeSet<ReplayUnit>,
        compiling: Instant,
    ) -> Result<Compiled, HotpatchError> {
        let plan = self.modified.record_change(&self.graph, units)?;
        let mut pending: BTreeSet<ReplayUnit> = plan.replay.into_iter().collect();
        pending.extend(self.dirty.iter().cloned());
        if pending.is_empty() {
            return Ok(Compiled::Nothing);
        }
        self.dirty = pending.clone();
        let order = self.graph.replay_order(&pending)?;
        let (image, libs): (Vec<ReplayUnit>, Vec<ReplayUnit>) =
            order.into_iter().partition(|unit| *unit == self.image_unit);

        let runner: &dyn ProcessRunner = &*self.runner;
        let outcomes = replay_units(runner, &self.graph, &self.records, &libs)?;
        for outcome in outcomes {
            if !outcome.success {
                // The units that compiled before this one keep their new
                // rlibs and stay ungated: the next candidate's layout
                // table is extracted from them too.
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
            self.ungated.compiled(outcome.unit, vec![rlib]);
        }
        if !image.is_empty() {
            let replayed = self.replay_image_unit()?;
            self.timings.compile = compiling.elapsed();
            match replayed {
                Ok(link_args) => {
                    let typed = if self.flavor == LinkerFlavor::Msvc {
                        // COFF objects carry CodeView, not DWARF: the gate
                        // reads the linked patch's PDB instead.
                        tip_objects(&link_args)
                    } else {
                        self.fresh_typed_objects(&tip_objects(&link_args))?
                    };
                    self.ungated.compiled(self.image_unit.clone(), typed);
                    self.tip_link_args = link_args;
                }
                Err(diagnostics) => return Ok(Compiled::Failed { diagnostics }),
            }
            self.dirty.remove(&self.image_unit);
        }

        // Every object compiled since the last accepted patch, this round's
        // and any an earlier round compiled before it failed, is gated here:
        // the patch links them all. Only an input the session accepted with
        // an earlier candidate is left out, and the accepted set holds it.
        // On Msvc the table is read after the link, from the patch's PDB,
        // which covers every ungated object the patch links
        // ([`PatchBuilder::linked_layouts`]). The objects read are chosen
        // by the base table's rule ([`gated_inputs`]).
        if self.timings.compile.is_zero() {
            self.timings.compile = compiling.elapsed();
        }
        let mut image_reads: BTreeMap<PathBuf, layout::InputTypes> =
            std::mem::take(&mut self.image_reads).into_iter().collect();
        let layouts = if self.flavor == LinkerFlavor::Msvc {
            LayoutTable::default()
        } else {
            let fresh = gated_inputs(self.ungated.objects(), &self.graph)?;
            if fresh.is_empty() {
                LayoutTable::default()
            } else {
                // The tip objects this round read already come with their
                // reads; every other input is read now.
                let (read, unread): (Vec<PathBuf>, Vec<PathBuf>) = fresh
                    .into_iter()
                    .partition(|input| image_reads.contains_key(input));
                let reads = read
                    .iter()
                    .filter_map(|input| image_reads.remove(input))
                    .collect();
                layout::extract_with(&unread, reads, &self.crates)?.table
            }
        };
        let seams = self.seam_set()?;
        self.timings.gate = compiling.elapsed().saturating_sub(self.timings.compile);
        Ok(Compiled::Candidate { layouts, seams })
    }

    /// [`PatchBuilder::link`], recording `symbols`, `link`, `table` and
    /// `strip`.
    fn link_timed(&mut self, n: u32, anchor_runtime: u64) -> Result<LinkedPatch, HotpatchError> {
        self.linked_layouts = None;
        let stubbing = Instant::now();
        let inputs = self.patch_inputs()?;
        let stub = super::stub::create_undefined_symbol_stub(&self.cache, &inputs, anchor_runtime)?;
        let stub_object = self.session_dir.join(format!("stub-{n}.o"));
        std::fs::write(&stub_object, stub).map_err(|err| {
            HotpatchError::io(format!("writing `{}`", stub_object.display()), err)
        })?;
        self.timings.symbols = stubbing.elapsed();
        let linking = Instant::now();
        let output = thin_link::patch_path(&self.target_dir, &self.session, n, self.flavor)?;
        let rlibs = self.modified_rlibs()?;
        // An Msvc patch links at its own fixed base: the runtime sees every
        // Windows image's slide as 0, so only an unrelocated DLL passes.
        let fixed_base = match self.flavor {
            LinkerFlavor::Msvc => Some(fat_link::patch_image_base(n).ok_or_else(|| {
                HotpatchError::unsupported(format!(
                    "patch {n} has no fixed Windows image base below the end of user space"
                ))
            })?),
            LinkerFlavor::Darwin | LinkerFlavor::Gnu => None,
        };
        let linked = thin_link::thin_link_with_base(
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
            fixed_base,
        )?;
        if self.flavor == LinkerFlavor::Msvc {
            let extraction =
                pdb_layout::extract(std::slice::from_ref(&linked.patch), &self.crates)?;
            self.linked_layouts = Some(extraction.table);
        }
        self.timings.link = linking.elapsed();
        read_linked(
            &*self.runner,
            &self.cache,
            self.target,
            linked.patch,
            self.upload_strip.as_deref(),
            &mut self.timings,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use frust_devtools_protocol::{HotpatchInfo, RpcError};

    use super::super::capture::prepare_scope_dir;
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
        /// The round compiles `app` and yields this candidate.
        Candidate(LayoutTable, SeamSet),
        /// The round compiles the libs named in `compiled`, then a later
        /// unit fails with `diagnostics`.
        Failed {
            compiled: Vec<&'static str>,
            diagnostics: Vec<String>,
        },
    }

    /// The object a fake round `round` produces for `name`.
    fn fake_rlib(round: u32, name: &str) -> PathBuf {
        PathBuf::from(format!("/fake/round-{round}/lib{name}.rlib"))
    }

    /// A builder whose compiles and links are scripted; `calls` records
    /// what the session asked of it (`accepted` for the acceptance hook).
    /// It keeps the real [`Ungated`] ledger, and `extracted` records the
    /// objects each candidate's layout table was extracted from.
    struct FakeBuilder {
        graph: Option<WorkspaceGraph>,
        compiles: VecDeque<FakeCompile>,
        calls: Arc<Mutex<Vec<String>>>,
        ungated: Arc<Mutex<Ungated>>,
        extracted: Arc<Mutex<Vec<Vec<PathBuf>>>>,
        rounds: u32,
        fail_link: bool,
        patch: Vec<u8>,
        /// Where `link` writes `patch-<n>.so` (the rig's session dir),
        /// world-readable, as a linker under a `022` umask would.
        out_dir: Option<PathBuf>,
    }

    impl FakeBuilder {
        fn new(compiles: Vec<FakeCompile>) -> Self {
            Self {
                graph: None,
                compiles: compiles.into(),
                calls: Arc::new(Mutex::new(Vec::new())),
                ungated: Arc::new(Mutex::new(Ungated::default())),
                extracted: Arc::new(Mutex::new(Vec::new())),
                rounds: 0,
                fail_link: false,
                patch: (0..1000u32).map(|i| (i % 253) as u8).collect(),
                out_dir: None,
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
            self.rounds += 1;
            let round = self.rounds;
            let mut ungated = self.ungated.lock().unwrap();
            let candidate = match self.compiles.pop_front() {
                Some(FakeCompile::Failed {
                    compiled,
                    diagnostics,
                }) => {
                    for name in compiled {
                        ungated.compiled(ReplayUnit::lib(name, name), vec![fake_rlib(round, name)]);
                    }
                    return Ok(Compiled::Failed { diagnostics });
                }
                Some(FakeCompile::Candidate(layouts, seams)) => (layouts, seams),
                None => (LayoutTable::default(), home_seam()),
            };
            ungated.compiled(ReplayUnit::lib("app", "app"), vec![fake_rlib(round, "app")]);
            self.extracted.lock().unwrap().push(ungated.inputs());
            Ok(Compiled::Candidate {
                layouts: candidate.0,
                seams: candidate.1,
            })
        }

        fn link(&mut self, n: u32, anchor_runtime: u64) -> Result<LinkedPatch, HotpatchError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("link {n} {anchor_runtime:#x}"));
            if self.fail_link {
                return Err(HotpatchError::unsupported("the thin link failed"));
            }
            let path = match &self.out_dir {
                Some(dir) => {
                    let path = dir.join(format!("patch-{n}.so"));
                    std::fs::write(&path, &self.patch).unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt as _;
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                            .unwrap();
                    }
                    path
                }
                None => PathBuf::from(format!("/nonexistent/patch-{n}.so")),
            };
            Ok(LinkedPatch {
                path,
                bytes: self.patch.clone(),
                table: fake_table(),
                symbols: patch_image(),
            })
        }

        fn accepted(&mut self) {
            self.calls.lock().unwrap().push("accepted".to_string());
            self.ungated.lock().unwrap().accept();
        }
    }

    /// The jump table every [`FakeBuilder`] patch links with.
    fn fake_table() -> JumpTableWire {
        JumpTableWire {
            map: [(0x40, 0x0)].into_iter().collect(),
            aslr_reference: 0x60,
            new_base_address: 0x20,
            ifunc_count: 0,
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
            patch_file_hand_off: false,
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
        ungated: Arc<Mutex<Ungated>>,
        extracted: Arc<Mutex<Vec<Vec<PathBuf>>>>,
        dir: PathBuf,
    }

    impl Rig {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        /// How many times the session called the acceptance hook.
        fn acceptances(&self) -> usize {
            self.calls().iter().filter(|c| *c == "accepted").count()
        }

        /// The builder's ungated objects, as the next candidate would read.
        fn ungated(&self) -> Vec<PathBuf> {
            self.ungated.lock().unwrap().inputs()
        }

        /// The objects each candidate's layout table was extracted from.
        fn extracted(&self) -> Vec<Vec<PathBuf>> {
            self.extracted.lock().unwrap().clone()
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
    fn rig_with(
        script: Script,
        mut builder: FakeBuilder,
        base: LayoutTable,
        budget: Budget,
    ) -> Rig {
        let server = test_server::spawn(script);
        let app = attach_app(server.addr, Some(FAKE_TOKEN), TRIPLE);
        let target_dir = temp_dir("rig");
        let accepted = AcceptedSets::begin(&target_dir, "session-app", base, home_seam()).unwrap();
        let calls = Arc::clone(&builder.calls);
        let ungated = Arc::clone(&builder.ungated);
        let extracted = Arc::clone(&builder.extracted);
        let dir = accepted.dir().to_path_buf();
        builder.out_dir = Some(dir.clone());
        Rig {
            session: HotSession {
                builder: Box::new(builder),
                app,
                accepted,
                images: vec![base_image()],
                budget,
                restart: None,
                next_patch_id: 1,
                timings: PatchTimings::default(),
                compiled: false,
                last_timings: None,
            },
            server,
            calls,
            ungated,
            extracted,
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
        assert_eq!(
            rig.calls(),
            vec!["compile 1", "link 1 0x100004000", "accepted"]
        );
        assert_eq!(
            rig.server.methods(),
            vec![
                "handshake",
                "hotpatch_info",
                "hotpatch_info",
                "patch_chunk",
                "table_chunk",
                "apply_patch"
            ]
        );
        let patch: Vec<u8> = (0..1000u32).map(|i| (i % 253) as u8).collect();
        assert_eq!(rig.server.uploaded(1), Some(patch));
        assert_eq!(rig.server.applied_table(1), Some(fake_table()));
        let apply = rig.server.requests().pop().unwrap();
        let params: ApplyPatchParams = serde_json::from_value(apply.params).unwrap();
        assert_eq!(
            (
                params.patch_id,
                params.len,
                params.pid,
                params.anchor_runtime,
                params.table_len,
                params.expected_seams
            ),
            (1, 1000, PID, ANCHOR, 16, 1)
        );
        assert!(outcome.to_string().starts_with("patched in "));
        assert!(outcome.to_string().ends_with(" ms (2 components rebuilt)"));
        assert_eq!(rig.sent(), vec!["patch_chunk", "apply_patch"]);
        assert_eq!(params.file, None, "no hand-off without the app's flag");
    }

    #[test]
    fn each_compiled_change_appends_one_timings_line_and_an_unaffected_one_none() {
        let mut builder = FakeBuilder::new(Vec::new());
        builder.graph = Some(graph_with_path_dependency());
        let mut rig = rig_with(
            hot_script(vec![applied(1, 2, Vec::new())]),
            builder,
            LayoutTable::default(),
            Budget::default(),
        );
        let log = rig.dir.join(PATCH_TIMINGS_FILE);
        assert_eq!(
            rig.session
                .on_change(&[PathBuf::from("/elsewhere/notes.md")]),
            Outcome::NoChange
        );
        assert!(rig.session.last_timings().is_none());
        assert!(!log.exists(), "nothing compiled, nothing timed");

        assert!(matches!(rig.change(), Outcome::Patched { .. }));
        let timings = *rig.session.last_timings().expect("the patch was timed");
        assert_eq!((timings.patch_bytes, timings.table_entries), (1000, 1));
        assert!(timings.total >= timings.apply + timings.upload_patch);
        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "{text}");
        assert!(
            lines[0].starts_with("hot patch timings: outcome=patched compile="),
            "{}",
            lines[0]
        );
        for field in [
            " gate=",
            " check=",
            " symbols=",
            " link=",
            " table=",
            " strip=",
            " upload_patch=",
            " upload_table=",
            " apply=",
            " total=",
            " patch_bytes=1000",
            " table_entries=1",
        ] {
            assert!(lines[0].contains(field), "{field} in {}", lines[0]);
        }
    }

    #[test]
    fn patch_timings_render_every_stage_in_milliseconds() {
        let timings = PatchTimings {
            compile: Duration::from_millis(7236),
            gate: Duration::from_millis(249),
            upload_patch: Duration::from_micros(1_304_900),
            total: Duration::from_millis(9858),
            patch_bytes: 17_843_800,
            table_entries: 68_692,
            ..PatchTimings::default()
        };
        assert_eq!(
            timings.to_string(),
            "compile=7236ms gate=249ms check=0ms symbols=0ms link=0ms table=0ms strip=0ms \
             upload_patch=1304ms upload_table=0ms apply=0ms total=9858ms patch_bytes=17843800 \
             table_entries=68692"
        );
    }

    #[test]
    fn an_app_advertising_the_hand_off_is_named_the_patch_file_and_sent_no_chunk() {
        let mut script = hot_script(vec![applied(1, 2, Vec::new())]);
        script.info.as_mut().unwrap().patch_file_hand_off = true;
        let mut rig = rig_with(
            script,
            FakeBuilder::new(Vec::new()),
            table(&[("app::HomeState", 4)]),
            Budget::default(),
        );
        let outcome = rig.change();
        assert!(
            matches!(outcome, Outcome::Patched { components: 2, .. }),
            "{outcome:?}"
        );
        assert_eq!(rig.sent(), vec!["apply_patch"]);
        assert_eq!(rig.server.uploaded(1), None);
        // The table still travels as chunks: no line carries it.
        assert_eq!(rig.server.applied_table(1), Some(fake_table()));

        let apply = rig.server.requests().pop().unwrap();
        let params: ApplyPatchParams = serde_json::from_value(apply.params).unwrap();
        assert_eq!((params.patch_id, params.len), (1, 1000));
        let file = params.file.expect("the hand-off names the patch file");
        let path = PathBuf::from(&file.path);
        assert!(path.is_absolute(), "{}", file.path);
        assert_eq!(
            path,
            std::path::absolute(rig.dir.join("patch-1.so")).unwrap()
        );
        let patch: Vec<u8> = (0..1000u32).map(|i| (i % 253) as u8).collect();
        assert_eq!(file.sha256.len(), 64);
        assert!(
            file.sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{}",
            file.sha256
        );
        assert_eq!(file.sha256, sha256_hex(&patch));
        assert_eq!(std::fs::read(&path).unwrap(), patch);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the hand-off file is owner-only");
        }
    }

    /// A device app (Android, reached through `adb forward`) is never named
    /// a host path: the forward makes its endpoint loopback on this host,
    /// but the app cannot read this host's files, so the patch is uploaded
    /// in chunks even when the app advertises the hand-off.
    #[test]
    fn a_device_session_uploads_even_when_the_app_advertises_the_hand_off() {
        let mut script = hot_script(vec![applied(1, 2, Vec::new())]);
        script.info.as_mut().unwrap().patch_file_hand_off = true;
        let server = test_server::spawn(script);
        let app = attach_device_app(server.addr, Some(FAKE_TOKEN), TRIPLE);
        assert!(
            matches!(
                app,
                AppLink::Live {
                    same_host: false,
                    ..
                }
            ),
            "a device attach is live and not on this host"
        );
        let mut builder = FakeBuilder::new(Vec::new());
        let calls = Arc::clone(&builder.calls);
        let accepted = AcceptedSets::begin(
            &temp_dir("device-hand-off"),
            "session-app",
            table(&[("app::HomeState", 4)]),
            home_seam(),
        )
        .unwrap();
        builder.out_dir = Some(accepted.dir().to_path_buf());
        let mut session = HotSession {
            builder: Box::new(builder),
            app,
            accepted,
            images: vec![base_image()],
            budget: Budget::default(),
            restart: None,
            next_patch_id: 1,
            timings: PatchTimings::default(),
            compiled: false,
            last_timings: None,
        };
        let outcome = session.on_change(&[PathBuf::from("/w/app/src/lib.rs")]);
        assert!(
            matches!(outcome, Outcome::Patched { components: 2, .. }),
            "{outcome:?}"
        );
        assert_eq!(
            calls.lock().unwrap().clone(),
            vec!["compile 1", "link 1 0x100004000", "accepted"]
        );
        let patch: Vec<u8> = (0..1000u32).map(|i| (i % 253) as u8).collect();
        assert_eq!(server.uploaded(1), Some(patch));
        let apply = server.requests().pop().unwrap();
        let params: ApplyPatchParams = serde_json::from_value(apply.params).unwrap();
        assert_eq!(params.file, None, "a device app is never named a host path");
    }

    /// Two sessions of one project on two devices (two TUI tabs, or two
    /// `frust run --watch -d <serial>`): each has its own session dir,
    /// starting the second leaves the first's patches in place, and each
    /// app is sent the patch linked against its own `anchor_runtime`.
    #[test]
    fn two_device_sessions_of_one_project_never_share_a_session_dir() {
        let target_dir = temp_dir("two-devices");
        let start = |serial: &str, anchor: u64, patch: Vec<u8>| {
            let mut script = hot_script(vec![applied(1, 2, Vec::new())]);
            script.info.as_mut().unwrap().anchor_runtime = anchor;
            let server = test_server::spawn(script);
            let app = attach_device_app(server.addr, Some(FAKE_TOKEN), TRIPLE);
            let mut builder = FakeBuilder::new(Vec::new());
            builder.patch = patch;
            let calls = Arc::clone(&builder.calls);
            let accepted = AcceptedSets::begin(
                &target_dir,
                &super::super::android::session_name("app", serial),
                table(&[("app::HomeState", 4)]),
                home_seam(),
            )
            .unwrap();
            builder.out_dir = Some(accepted.dir().to_path_buf());
            let session = HotSession {
                builder: Box::new(builder),
                app,
                accepted,
                images: vec![base_image()],
                budget: Budget::default(),
                restart: None,
                next_patch_id: 1,
                timings: PatchTimings::default(),
                compiled: false,
                last_timings: None,
            };
            (session, server, calls)
        };
        let change = |session: &mut HotSession| {
            let outcome = session.on_change(&[PathBuf::from("/w/app/src/lib.rs")]);
            assert!(matches!(outcome, Outcome::Patched { .. }), "{outcome:?}");
        };
        let patch_a: Vec<u8> = (0..1000u32).map(|i| (i % 253) as u8).collect();
        let patch_b: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();

        let (mut a, server_a, calls_a) = start("0A1B2C3D4E5F", ANCHOR, patch_a.clone());
        let dir_a = a.accepted.dir().to_path_buf();
        change(&mut a);
        assert_eq!(std::fs::read(dir_a.join("patch-1.so")).unwrap(), patch_a);

        let (mut b, server_b, calls_b) =
            start("192.168.1.5:5555", ANCHOR + 0x8000, patch_b.clone());
        let dir_b = b.accepted.dir().to_path_buf();
        assert_ne!(dir_a, dir_b);
        assert_eq!(dir_a.parent(), dir_b.parent());
        assert_eq!(
            std::fs::read(dir_a.join("patch-1.so")).unwrap(),
            patch_a,
            "starting the second session leaves the first's dir intact"
        );
        assert!(dir_a.join(layout::LAYOUT_BASE_FILE).exists());
        assert!(dir_a.join(LAYOUTS_ACCEPTED_FILE).exists());
        assert!(!dir_b.join("patch-1.so").exists());

        change(&mut b);
        assert_eq!(std::fs::read(dir_a.join("patch-1.so")).unwrap(), patch_a);
        assert_eq!(std::fs::read(dir_b.join("patch-1.so")).unwrap(), patch_b);
        assert_eq!(
            calls_a.lock().unwrap().clone(),
            vec!["compile 1", "link 1 0x100004000", "accepted"]
        );
        assert_eq!(
            calls_b.lock().unwrap().clone(),
            vec!["compile 1", "link 1 0x10000c000", "accepted"]
        );
        assert_eq!(server_a.uploaded(1), Some(patch_a));
        assert_eq!(server_b.uploaded(1), Some(patch_b));
    }

    /// What the stand-in `llvm-strip` writes: no symbol table at all, so a
    /// jump table could never be built from it.
    const STRIPPED: &[u8] = b"stripped upload copy";

    /// A runner standing in for `llvm-strip`: writes [`STRIPPED`] to the
    /// path after `-o`, world-readable as under a `022` umask, and records
    /// every argv.
    #[derive(Default)]
    struct FakeStrip {
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl ProcessRunner for FakeStrip {
        fn run(&self, cmd: &str, _args: &[&str]) -> anyhow::Result<Output> {
            anyhow::bail!("FakeStrip does not run `{cmd}`")
        }

        fn run_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            _cwd: Option<&Path>,
            _env: &[(&str, &str)],
            _on_line: &mut dyn FnMut(&str),
        ) -> anyhow::Result<Output> {
            let argv: Vec<String> = std::iter::once(cmd)
                .chain(args.iter().copied())
                .map(str::to_string)
                .collect();
            let out = args
                .iter()
                .position(|arg| *arg == "-o")
                .map(|i| PathBuf::from(args[i + 1]))
                .expect("llvm-strip is given -o");
            std::fs::write(&out, STRIPPED)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o644))?;
            }
            self.calls.lock().unwrap().push(argv);
            Ok(Output {
                success: true,
                ..Output::default()
            })
        }

        fn spawn_streaming(
            &self,
            cmd: &str,
            _args: &[&str],
            _cwd: Option<&Path>,
            _env: &[(&str, &str)],
        ) -> anyhow::Result<StreamHandle> {
            anyhow::bail!("FakeStrip does not spawn `{cmd}`")
        }
    }

    fn android() -> Target {
        Target::from_triple("aarch64-linux-android").unwrap()
    }

    /// An unstripped Android patch: the seam the base also defines, moved.
    fn android_patch() -> Vec<u8> {
        object(
            android(),
            &[Def::Text("seam_home", 32), Def::Text(ANCHOR_SYMBOL, 4)],
        )
    }

    /// A builder whose link step is the real [`read_linked`] over a
    /// scripted thin link: `link` writes [`android_patch`] to `patch-<n>.so`
    /// owner-only, as `thin_link` leaves it.
    struct ReadLinkedBuilder {
        runner: Arc<FakeStrip>,
        cache: SymbolCache,
        dir: PathBuf,
        upload_strip: Option<PathBuf>,
    }

    impl PatchBuilder for ReadLinkedBuilder {
        fn classify(&self, _path: &Path) -> PathClass {
            PathClass::Replayable {
                units: [ReplayUnit::lib("app", "app")].into_iter().collect(),
            }
        }

        fn compile(&mut self, _units: &BTreeSet<ReplayUnit>) -> Result<Compiled, HotpatchError> {
            Ok(Compiled::Candidate {
                layouts: LayoutTable::default(),
                seams: home_seam(),
            })
        }

        fn link(&mut self, n: u32, _anchor_runtime: u64) -> Result<LinkedPatch, HotpatchError> {
            let patch = self.dir.join(format!("patch-{n}.so"));
            std::fs::write(&patch, android_patch()).unwrap();
            thin_link::restrict_to_owner(&patch)?;
            read_linked(
                &*self.runner,
                &self.cache,
                android(),
                patch,
                self.upload_strip.as_deref(),
                &mut PatchTimings::default(),
            )
        }

        fn accepted(&mut self) {}
    }

    /// A session over a [`ReadLinkedBuilder`] whose base is an Android
    /// image, attached to a fake app on this host or (`same_host: false`)
    /// on a device.
    struct LinkedRig {
        session: HotSession,
        server: FakeServer,
        runner: Arc<FakeStrip>,
        dir: PathBuf,
        /// The jump table the unstripped patch gives against the base.
        expected_table: JumpTableWire,
    }

    fn linked_rig(
        script: Script,
        same_host: bool,
        upload_strip: Option<PathBuf>,
        budget: Budget,
    ) -> LinkedRig {
        let target_dir = temp_dir("read-linked");
        let base = target_dir.join("libapp.so");
        std::fs::write(
            &base,
            object(
                android(),
                &[
                    Def::Text("pad", 64),
                    Def::Text("seam_home", 16),
                    Def::Text(ANCHOR_SYMBOL, 4),
                ],
            ),
        )
        .unwrap();
        let cache = SymbolCache::load(&base, android()).unwrap();
        let patch = ImageSymbols::parse(&android_patch(), android(), "patch").unwrap();
        let expected_table = super::super::jump_table::create_jump_table(&cache, &patch).unwrap();
        let server = test_server::spawn(script);
        let app = attach(server.addr, Some(FAKE_TOKEN), TRIPLE, same_host);
        let accepted = AcceptedSets::begin(
            &target_dir,
            "session-app",
            LayoutTable::default(),
            home_seam(),
        )
        .unwrap();
        let dir = accepted.dir().to_path_buf();
        let runner = Arc::new(FakeStrip::default());
        let images = vec![cache.symbols().clone()];
        let builder = ReadLinkedBuilder {
            runner: Arc::clone(&runner),
            cache,
            dir: dir.clone(),
            upload_strip,
        };
        LinkedRig {
            session: HotSession {
                builder: Box::new(builder),
                app,
                accepted,
                images,
                budget,
                restart: None,
                next_patch_id: 1,
                timings: PatchTimings::default(),
                compiled: false,
                last_timings: None,
            },
            server,
            runner,
            dir,
            expected_table,
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// An Android session uploads the stripped copy; the jump table and the
    /// image symbols the session keeps come from the unstripped patch,
    /// which stays in the session dir. Both are owner-only. `len` and the
    /// byte budget count the uploaded bytes: a budget the stripped copy
    /// exactly fills admits the patch, though the unstripped image is over.
    #[test]
    fn a_device_session_uploads_the_stripped_copy_and_keeps_the_unstripped_patch() {
        let budget = Budget {
            patches: DEFAULT_BUDGET_PATCHES,
            bytes: STRIPPED.len() as u64,
        };
        assert!(android_patch().len() as u64 > budget.bytes);
        let strip = PathBuf::from("/ndk/bin/llvm-strip");
        let mut rig = linked_rig(
            hot_script(vec![applied(1, 2, Vec::new())]),
            false,
            Some(strip.clone()),
            budget,
        );
        let outcome = rig.session.on_change(&[PathBuf::from("/w/app/src/lib.rs")]);
        assert!(
            matches!(outcome, Outcome::Patched { components: 2, .. }),
            "{outcome:?}"
        );

        let patch = rig.dir.join("patch-1.so");
        let upload = rig.dir.join("patch-1.upload.so");
        let path = |p: &Path| p.to_string_lossy().into_owned();
        assert_eq!(
            rig.runner.calls.lock().unwrap().clone(),
            vec![vec![
                path(&strip),
                "--strip-unneeded".to_string(),
                "-o".to_string(),
                path(&upload),
                path(&patch),
            ]]
        );
        assert_eq!(rig.server.uploaded(1), Some(STRIPPED.to_vec()));
        let apply = rig.server.requests().pop().unwrap();
        let params: ApplyPatchParams = serde_json::from_value(apply.params).unwrap();
        assert_eq!(params.len, STRIPPED.len() as u64);
        assert_eq!(params.file, None);
        assert_eq!(params.table_len, rig.expected_table.encoded_map_len());
        assert_eq!(
            rig.server.applied_table(1),
            Some(rig.expected_table.clone())
        );
        let kept = &rig.session.images[1];
        let unstripped = ImageSymbols::parse(&android_patch(), android(), "patch").unwrap();
        assert!(kept.defined_address("seam_home").is_some());
        assert_eq!(
            kept.defined_address("seam_home"),
            unstripped.defined_address("seam_home")
        );

        assert_eq!(std::fs::read(&patch).unwrap(), android_patch());
        assert_eq!(std::fs::read(&upload).unwrap(), STRIPPED);
        #[cfg(unix)]
        {
            assert_eq!(mode(&patch), 0o600, "the unstripped patch is owner-only");
            assert_eq!(mode(&upload), 0o600, "the upload copy is owner-only");
        }
    }

    /// Any digest describes the bytes the app is sent: a hand-off of a
    /// stripped patch names the upload copy and its SHA-256.
    #[test]
    fn a_hand_off_of_a_stripped_patch_names_and_digests_the_upload_copy() {
        let mut script = hot_script(vec![applied(1, 2, Vec::new())]);
        script.info.as_mut().unwrap().patch_file_hand_off = true;
        let mut rig = linked_rig(
            script,
            true,
            Some(PathBuf::from("/ndk/bin/llvm-strip")),
            Budget::default(),
        );
        let outcome = rig.session.on_change(&[PathBuf::from("/w/app/src/lib.rs")]);
        assert!(matches!(outcome, Outcome::Patched { .. }), "{outcome:?}");
        let apply = rig.server.requests().pop().unwrap();
        let params: ApplyPatchParams = serde_json::from_value(apply.params).unwrap();
        let file = params.file.expect("the hand-off names a file");
        assert_eq!(
            PathBuf::from(&file.path),
            std::path::absolute(rig.dir.join("patch-1.upload.so")).unwrap()
        );
        assert_eq!(file.sha256, sha256_hex(STRIPPED));
        assert_eq!(params.len, STRIPPED.len() as u64);
    }

    /// Without a strip tool (every desktop session) nothing is run and the
    /// linked patch itself is uploaded, or named and digested on a
    /// hand-off, exactly as linked.
    #[test]
    fn without_a_strip_tool_the_linked_patch_is_sent_as_linked() {
        for hand_off in [false, true] {
            let mut script = hot_script(vec![applied(1, 2, Vec::new())]);
            script.info.as_mut().unwrap().patch_file_hand_off = hand_off;
            let mut rig = linked_rig(script, true, None, Budget::default());
            let outcome = rig.session.on_change(&[PathBuf::from("/w/app/src/lib.rs")]);
            assert!(matches!(outcome, Outcome::Patched { .. }), "{outcome:?}");
            assert!(rig.runner.calls.lock().unwrap().is_empty());
            assert!(!rig.dir.join("patch-1.upload.so").exists());
            let apply = rig.server.requests().pop().unwrap();
            let params: ApplyPatchParams = serde_json::from_value(apply.params).unwrap();
            assert_eq!(params.len, android_patch().len() as u64);
            assert_eq!(
                rig.server.applied_table(1),
                Some(rig.expected_table.clone())
            );
            match params.file {
                Some(file) => {
                    assert!(hand_off);
                    assert_eq!(
                        PathBuf::from(&file.path),
                        std::path::absolute(rig.dir.join("patch-1.so")).unwrap()
                    );
                    assert_eq!(file.sha256, sha256_hex(&android_patch()));
                    assert_eq!(rig.server.uploaded(1), None);
                }
                None => {
                    assert!(!hand_off);
                    assert_eq!(rig.server.uploaded(1), Some(android_patch()));
                }
            }
        }
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
            timings: PatchTimings::default(),
            compiled: false,
            last_timings: None,
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
            vec!["compile 1", "link 1 0x100004000", "accepted", "compile 1"]
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

    /// [`graph_with_path_dependency`] after a desktop fat build captured
    /// `frust-material`'s lib.
    fn graph_with_captured_path_dependency() -> WorkspaceGraph {
        let mut graph = graph_with_path_dependency();
        let record = |ty: &str| RustcRecord {
            args: vec!["rustc".to_string()],
            envs: Vec::new(),
            crate_types: vec![ty.to_string()],
        };
        let records = [
            (RecordKey::parse("app.lib").unwrap(), record("lib")),
            (RecordKey::parse("app.bin").unwrap(), record("bin")),
            (
                RecordKey::parse("frust_material.lib").unwrap(),
                record("lib"),
            ),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            graph.replay_non_members(&records),
            vec!["frust-material".to_string()]
        );
        graph
    }

    #[test]
    fn a_captured_path_dependency_change_is_replayed_and_patched() {
        let mut builder = FakeBuilder::new(Vec::new());
        builder.graph = Some(graph_with_captured_path_dependency());
        let mut rig = rig_with(
            hot_script(vec![applied(1, 1, Vec::new())]),
            builder,
            table(&[("frust_material::Theme", 8)]),
            Budget::default(),
        );
        let outcome = rig
            .session
            .on_change(&[PathBuf::from("/x/material/src/button.rs")]);
        assert!(matches!(outcome, Outcome::Patched { .. }), "{outcome:?}");
        assert_eq!(
            rig.calls(),
            vec!["compile 1", "link 1 0x100004000", "accepted"]
        );
        assert_eq!(rig.sent(), vec!["patch_chunk", "apply_patch"]);
    }

    #[test]
    fn a_captured_path_dependency_layout_change_restarts_before_anything_is_sent() {
        let mut builder = FakeBuilder::new(vec![FakeCompile::Candidate(
            table(&[("frust_material::Theme", 16)]),
            home_seam(),
        )]);
        builder.graph = Some(graph_with_captured_path_dependency());
        let mut rig = rig_with(
            hot_script(Vec::new()),
            builder,
            table(&[("frust_material::Theme", 8)]),
            Budget::default(),
        );
        let reason = restart_reason(
            rig.session
                .on_change(&[PathBuf::from("/x/material/src/theme.rs")]),
        );
        assert!(
            matches!(&reason, RestartReason::LayoutChanged { records }
                if records.iter().any(|r| r.contains("frust_material::Theme"))),
            "{reason:?}"
        );
        assert_eq!(rig.calls(), vec!["compile 1"], "no link");
        assert!(rig.sent().is_empty(), "nothing sent");
    }

    #[test]
    fn an_uncaptured_path_dependency_change_restarts_with_no_thin_build() {
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
    fn the_link_line_helpers_cover_a_captured_path_dependency() {
        let graph = graph_with_captured_path_dependency();
        let args: Vec<String> = [
            "/t/debug/deps/app-1.app.a-cgu.0.rcgu.o",
            "/t/debug/deps/libapp-77.rlib",
            "/t/debug/deps/libfrust_material-9.rlib",
            "/rustlib/libstd-1.rlib",
        ]
        .map(str::to_string)
        .to_vec();
        assert_eq!(
            member_rlibs(&args, &graph),
            [
                (
                    ReplayUnit::lib("app", "app"),
                    PathBuf::from("/t/debug/deps/libapp-77.rlib")
                ),
                (
                    ReplayUnit::lib("frust-material", "frust_material"),
                    PathBuf::from("/t/debug/deps/libfrust_material-9.rlib")
                ),
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(
            replayable_crates(&graph),
            vec!["app".to_string(), "frust_material".to_string()],
            "the layout gate covers the path dependency's types"
        );
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
        // Round 1: `core` compiles, a later unit fails. Round 2 compiles
        // `app` and is applied; round 3 likewise.
        let mut rig = rig(
            vec![applied(1, 1, Vec::new()), applied(2, 1, Vec::new())],
            vec![FakeCompile::Failed {
                compiled: vec!["core"],
                diagnostics: vec!["error[E0308]".into()],
            }],
        );
        assert_eq!(
            rig.change(),
            Outcome::CompileFailed {
                diagnostics: vec!["error[E0308]".into()]
            }
        );
        assert!(rig.sent().is_empty());
        assert_eq!(rig.ungated(), vec![fake_rlib(1, "core")]);
        assert!(matches!(rig.change(), Outcome::Patched { .. }));
        // The failed round's `core` reached the next candidate's layout
        // extraction beside that round's own `app`.
        assert_eq!(
            rig.extracted(),
            vec![vec![fake_rlib(2, "app"), fake_rlib(1, "core")]]
        );
        assert!(rig.ungated().is_empty(), "the applied patch gated them");
        assert!(matches!(rig.change(), Outcome::Patched { .. }));
        assert_eq!(rig.extracted()[1], vec![fake_rlib(3, "app")]);
        assert_eq!(rig.acceptances(), 2);
    }

    #[test]
    fn the_acceptance_hook_runs_once_per_applied_patch_and_never_on_a_refusal() {
        // Two applied patches: one hook call each, after the link.
        let mut r = rig(
            vec![applied(1, 1, Vec::new()), applied(2, 1, Vec::new())],
            Vec::new(),
        );
        assert!(matches!(r.change(), Outcome::Patched { .. }));
        assert!(matches!(r.change(), Outcome::Patched { .. }));
        assert_eq!(
            r.calls(),
            vec![
                "compile 1",
                "link 1 0x100004000",
                "accepted",
                "compile 1",
                "link 2 0x100004000",
                "accepted"
            ]
        );
        assert!(r.ungated().is_empty());

        // Applied and merged, then a restart for no seam hit: the patch is
        // live and its entries accepted, so the hook ran.
        let mut r = rig(vec![applied(1, 0, Vec::new())], Vec::new());
        assert_eq!(restart_reason(r.change()), RestartReason::NoSeamHit);
        assert_eq!(r.acceptances(), 1);

        // Every refusal and failure leaves the hook uncalled and the
        // compiled objects ungated.
        let mismatch = PatchOutcome {
            applied: true,
            seam_hits: 1,
            seam_fall_throughs: Vec::new(),
            layout_mismatches: vec!["app::HomeState: 4 -> 8 bytes".into()],
            patches_applied: 1,
            patch_bytes_loaded: 1000,
        };
        let not_applied = PatchOutcome {
            applied: false,
            layout_mismatches: Vec::new(),
            patches_applied: 0,
            ..mismatch.clone()
        };
        let refused_by_the_gate = vec![FakeCompile::Candidate(
            table(&[("app::HomeState", 8)]),
            home_seam(),
        )];
        let cases: Vec<(&str, Vec<ApplyReply>, Vec<FakeCompile>, bool)> = vec![
            ("host layout gate", Vec::new(), refused_by_the_gate, false),
            ("failed link", Vec::new(), Vec::new(), true),
            (
                "layout-mismatch record",
                vec![ApplyReply::Outcome(mismatch)],
                Vec::new(),
                false,
            ),
            (
                "applied: false",
                vec![ApplyReply::Outcome(not_applied)],
                Vec::new(),
                false,
            ),
            ("lost reply", vec![ApplyReply::Hangup], Vec::new(), false),
            (
                "error reply",
                vec![ApplyReply::Error(RpcError::new(
                    RpcError::INTERNAL_ERROR,
                    "patch not applied",
                ))],
                Vec::new(),
                false,
            ),
        ];
        for (case, applies, compiles, fail_link) in cases {
            let mut builder = FakeBuilder::new(compiles);
            builder.fail_link = fail_link;
            let mut r = rig_with(
                hot_script(applies),
                builder,
                table(&[("app::HomeState", 4)]),
                Budget::default(),
            );
            restart_reason(r.change());
            assert_eq!(r.acceptances(), 0, "{case}: {:?}", r.calls());
            assert_eq!(r.ungated(), vec![fake_rlib(1, "app")], "{case}");
        }

        // A layout mismatch pending in the app before the send.
        let mut script = hot_script(Vec::new());
        // The attach reads the first answer, the change the second.
        script.pending = [
            Vec::new(),
            vec!["app::HomeState changed layout".to_string()],
        ]
        .into();
        let mut r = rig_with(
            script,
            FakeBuilder::new(Vec::new()),
            table(&[("app::HomeState", 4)]),
            Budget::default(),
        );
        assert!(matches!(
            restart_reason(r.change()),
            RestartReason::LayoutChanged { .. }
        ));
        assert_eq!(r.acceptances(), 0);
        assert_eq!(r.ungated(), vec![fake_rlib(1, "app")]);
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
            vec!["compile 1", "link 1 0x100004000", "accepted", "compile 1"]
        );
    }

    /// A [`FakeBuilder`] whose candidate tables appear only after each
    /// link, as a PE patch's PDB table does.
    struct PostLinkTables {
        inner: FakeBuilder,
        tables: VecDeque<LayoutTable>,
        linked: Option<LayoutTable>,
    }

    impl PatchBuilder for PostLinkTables {
        fn classify(&self, path: &Path) -> PathClass {
            self.inner.classify(path)
        }

        fn compile(&mut self, units: &BTreeSet<ReplayUnit>) -> Result<Compiled, HotpatchError> {
            self.inner.compile(units)
        }

        fn link(&mut self, n: u32, anchor_runtime: u64) -> Result<LinkedPatch, HotpatchError> {
            let linked = self.inner.link(n, anchor_runtime)?;
            self.linked = self.tables.pop_front();
            Ok(linked)
        }

        fn linked_layouts(&mut self) -> Option<LayoutTable> {
            self.linked.take()
        }

        fn accepted(&mut self) {
            self.inner.accepted();
        }
    }

    #[test]
    fn msvc_seams_are_read_from_coff_objects_and_rlib_members() {
        // `HomePage`'s seam instance, v0-mangled as rustc spells it.
        const SEAM: &str = "_RNvXNtCsfxYKN7w6hnv_14frust_hotpatch6hot_fnINvNtCskhDZPiJ344Y_10frust_core8hotpatch13checked_buildNtCskJOlR6liO6y_20hotpatch_fixture_app8HomePageEINtB2_11HotFunctionTRB1y_QNtB1A_9HomeStateNtBI_11SeamWitnessENtB2_9Fn3MarkerE7call_itB1A_";
        let windows = Target::from_triple("x86_64-pc-windows-msvc").unwrap();
        let dir = temp_dir("coff-seams");
        let loose = dir.join("app.app.aaaa-cgu.0.rcgu.o");
        std::fs::write(
            &loose,
            object(windows, &[Def::Text("plain_fn", 4), Def::Undefined("ext")]),
        )
        .unwrap();
        let rlib = dir.join("libapp-1.rlib");
        let member = object(windows, &[Def::Text(SEAM, 8)]);
        let mut builder = ar::Builder::new(Vec::new());
        for (name, bytes) in [
            ("lib.rmeta", &b"meta"[..]),
            ("app-1.app.bbbb-cgu.0.rcgu.o", &member[..]),
        ] {
            let header = ar::Header::new(name.as_bytes().to_vec(), bytes.len() as u64);
            builder.append(&header, bytes).unwrap();
        }
        std::fs::write(&rlib, builder.into_inner().unwrap()).unwrap();

        let set = super::seam_set(LinkerFlavor::Msvc, &[loose.clone(), rlib.clone()]).unwrap();
        assert_eq!(set, SeamSet::from_symbols([SEAM]).unwrap());
        assert_eq!(set.len(), 1);

        // Another format, or an archive with no object, fails closed.
        let elf = dir.join("elf.o");
        std::fs::write(&elf, object(target(), &[Def::Text("x", 4)])).unwrap();
        for bad in [elf, dir.join("empty.rlib")] {
            if !bad.exists() {
                std::fs::write(&bad, b"!<arch>\n").unwrap();
            }
            let err = super::seam_set(LinkerFlavor::Msvc, &[bad]).unwrap_err();
            assert!(
                matches!(err, HotpatchError::BuilderUnsupported { .. }),
                "{err:?}"
            );
        }
    }

    #[test]
    fn a_post_link_table_is_gated_before_sending_and_merged_once_applied() {
        // Each candidate compiles with no table (a PE target's objects carry
        // none the gate reads); its table appears with the linked patch.
        let mut inner = FakeBuilder::new(vec![
            FakeCompile::Candidate(LayoutTable::default(), home_seam()),
            FakeCompile::Candidate(LayoutTable::default(), home_seam()),
        ]);
        let server = test_server::spawn(hot_script(vec![applied(1, 1, Vec::new())]));
        let app = attach_app(server.addr, Some(FAKE_TOKEN), TRIPLE);
        let accepted = AcceptedSets::begin(
            &temp_dir("post-link"),
            "session-app",
            table(&[("app::HomeState", 4)]),
            home_seam(),
        )
        .unwrap();
        inner.out_dir = Some(accepted.dir().to_path_buf());
        let calls = Arc::clone(&inner.calls);
        let mut session = HotSession {
            builder: Box::new(PostLinkTables {
                inner,
                tables: [
                    table(&[("app::HomeState", 4), ("app::Badge", 4)]),
                    table(&[("app::HomeState", 4), ("app::Badge", 8)]),
                ]
                .into(),
                linked: None,
            }),
            app,
            accepted,
            images: vec![base_image()],
            budget: Budget::default(),
            restart: None,
            next_patch_id: 1,
            timings: PatchTimings::default(),
            compiled: false,
            last_timings: None,
        };
        let change =
            |session: &mut HotSession| session.on_change(&[PathBuf::from("/w/app/src/lib.rs")]);

        assert!(matches!(change(&mut session), Outcome::Patched { .. }));
        assert_eq!(
            session
                .accepted()
                .layouts()
                .get("app::Badge")
                .map(|e| e.size),
            Some(4),
            "the post-link table is what an applied patch merges"
        );
        let reason = restart_reason(change(&mut session));
        assert_eq!(
            reason,
            RestartReason::LayoutChanged {
                records: vec!["app::Badge changed layout (4 → 8 bytes)".into()]
            }
        );
        let sent: Vec<String> = server
            .methods()
            .into_iter()
            .skip(2)
            .filter(|m| m == "patch_chunk" || m == "apply_patch")
            .collect();
        assert_eq!(
            sent,
            vec!["patch_chunk", "apply_patch"],
            "patch 2 is never sent"
        );
        assert_eq!(
            calls.lock().unwrap().clone(),
            vec![
                "compile 1",
                "link 1 0x100004000",
                "accepted",
                "compile 1",
                "link 2 0x100004000"
            ],
            "the refusal comes after the second link"
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
    fn a_state_identity_change_wins_over_layout_records() {
        // Patch 1 adds `Badge`; patch 2 grows it (a layout record) AND
        // changes `Home`'s State identity: the identity reason is reported.
        let changed = seam_set(&[("app::Home", "(&app::Home, &mut app::OtherState)", "_x")]);
        let mut rig = rig(
            vec![applied(1, 1, Vec::new())],
            vec![
                FakeCompile::Candidate(table(&[("app::Badge", 4)]), home_seam()),
                FakeCompile::Candidate(table(&[("app::Badge", 8)]), changed),
            ],
        );
        assert!(matches!(rig.change(), Outcome::Patched { .. }));
        let sent = rig.sent().len();
        let reason = restart_reason(rig.change());
        assert!(
            matches!(reason, RestartReason::StateTypeChanged { .. }),
            "{reason:?}"
        );
        assert_eq!(rig.sent().len(), sent, "nothing is sent for patch 2");
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
                .all(|call| call.starts_with("cargo metadata") || call == "rustc -vV"),
            "nothing but the toolchain probe and metadata may run: {invocations:?}"
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
        // The ambient list comes last and names this process's whole
        // environment plus every pair before it.
        let (ambient, env) = env.split_last().expect("the ambient list");
        assert_eq!(ambient.0, "FRUST_HOTPATCH_AMBIENT");
        let ambient_names: Vec<&str> = ambient.1.lines().collect();
        for name in env.iter().map(|(name, _)| name.as_str()).chain(["PATH"]) {
            assert!(
                ambient_names.contains(&name),
                "{name} missing from {ambient_names:?}"
            );
        }
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
                "rustc -vV".to_string(),
                format!(
                    "cargo metadata --format-version 1 --manifest-path {} --filter-platform {TRIPLE}",
                    root.join("Cargo.toml").display()
                ),
            ],
            "the graph is the host triple's, read before anything is built"
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
    fn a_desktop_start_captures_a_path_dependency_through_rustc_wrapper() {
        let (root, target_dir, _) = project("path-dep-start", Output::default());
        let widgets = root.parent().unwrap().join("frust").join("widgets");
        const APP: &str = "path+file:///p/my-app#0.1.0";
        const WIDGETS: &str = "path+file:///p/frust/widgets#0.6.0";
        let metadata = serde_json::json!({
            "packages": [
                {"id": APP, "name": "my-app", "source": null,
                 "manifest_path": root.join("Cargo.toml"),
                 "targets": [
                    {"name": "my_app", "kind": ["lib"], "crate_types": ["lib"],
                     "src_path": root.join("src/lib.rs")},
                    {"name": "my-app", "kind": ["bin"], "crate_types": ["bin"],
                     "src_path": root.join("src/main.rs")},
                 ]},
                {"id": WIDGETS, "name": "frust-widgets", "source": null,
                 "manifest_path": widgets.join("Cargo.toml"),
                 "targets": [{"name": "frust_widgets", "kind": ["lib"], "crate_types": ["lib"],
                              "src_path": widgets.join("src/lib.rs")}]},
            ],
            "workspace_members": [APP],
            "resolve": {"nodes": [
                {"id": APP, "deps": [{"name": "frust_widgets", "pkg": WIDGETS,
                                      "dep_kinds": [{"kind": null, "target": null}]}]},
                {"id": WIDGETS, "deps": []},
            ], "root": APP},
            "workspace_root": root,
            "target_directory": target_dir,
        });
        let ok = |stdout: String| Output {
            success: true,
            stdout,
            stderr: String::new(),
        };
        let runner = Arc::new(ScriptedRunner {
            runs: [
                ("cargo metadata".to_string(), ok(metadata.to_string())),
                ("rustc -vV".to_string(), ok(RUSTC_VV.to_string())),
            ]
            .into_iter()
            .collect(),
            streaming: Output {
                success: true,
                ..Output::default()
            },
            calls: Mutex::new(Vec::new()),
        });
        let fingerprints = target_dir.join("debug").join(".fingerprint");
        for name in ["frust-widgets-1111", "serde-2222"] {
            std::fs::create_dir_all(fingerprints.join(name)).unwrap();
        }

        start(
            &runner,
            &FakeEnv::new(),
            &root,
            &debug_info(),
            &mut Vec::new(),
        );

        let (_, env, _) = runner.call("cargo");
        let value = |name: &str| {
            env.iter()
                .find(|(set, _)| set == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(
            value("RUSTC_WRAPPER").as_deref(),
            Some("/opt/frust/bin/frust")
        );
        assert_eq!(
            value("RUSTC_WORKSPACE_WRAPPER").as_deref(),
            Some("/opt/frust/bin/frust")
        );
        let scope = PathBuf::from(value("FRUST_HOTPATCH_CAPTURE").unwrap());
        let listed = capture::read_non_members(&scope).unwrap().unwrap();
        assert_eq!(listed.names(), vec!["frust-widgets".to_string()]);
        assert!(listed.contains_dir(&widgets));
        let member_only = ScopeInputs {
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
        assert_ne!(
            scope.file_name().unwrap().to_string_lossy(),
            member_only.dir_name().unwrap(),
            "a non-member scope is never a member-only one"
        );
        assert!(
            !fingerprints.join("frust-widgets-1111").exists(),
            "an uncaptured path dependency is busted so the wrapper sees it"
        );
        assert!(fingerprints.join("serde-2222").is_dir());
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
            WrapperSetup {
                frust_exe: Path::new("/bin/frust"),
                scope_dir: Path::new("/t/scope"),
                ambient_names: &["PATH".to_string(), "HOME".to_string()],
            },
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
                // The host's names plus every name added above: what cargo
                // starts from, so the wrapper can tell its additions apart.
                (
                    "FRUST_HOTPATCH_AMBIENT".to_string(),
                    "APP_FLAVOR\nFRUST_HOTPATCH_AMBIENT\nFRUST_HOTPATCH_CAPTURE\n\
                     FRUST_HOTPATCH_LINK\nFRUST_HOTPATCH_LINK_ARGS_FILE\nHOME\nPATH\n\
                     RUSTC_WORKSPACE_WRAPPER"
                        .to_string()
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
    fn linked_crates_names_every_rlib_on_the_link_line() {
        let args: Vec<String> = [
            "/t/debug/deps/app-1.app.a-cgu.0.rcgu.o",
            "/t/debug/deps/libapp-77.rlib",
            "/t/debug/deps/libfrust_shell_ios-9.rlib",
            "/t/debug/deps/libplain.rlib",
            "/t/debug/deps/libnot_an_rlib-1.rmeta",
        ]
        .map(str::to_string)
        .to_vec();
        assert_eq!(
            linked_crates(&args),
            ["app", "frust_shell_ios", "plain"]
                .map(str::to_string)
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn an_empty_path_dependency_rlib_is_left_out_of_the_base_table_but_never_a_members() {
        let dir = temp_dir("gated-rlibs");
        let empty = super::super::link_intercept::empty_object(
            object::BinaryFormat::MachO,
            object::Architecture::Aarch64,
        )
        .unwrap();
        let empty_object = dir.join("empty.o");
        std::fs::write(&empty_object, &empty).unwrap();
        let code_object = dir.join("code.o");
        std::fs::write(&code_object, object(target(), &[Def::Text("f", 8)])).unwrap();
        assert!(!layout::holds_code_or_data(&empty_object).unwrap());
        assert!(layout::holds_code_or_data(&code_object).unwrap());

        let graph = graph_with_captured_path_dependency();
        let app = ReplayUnit::lib("app", "app");
        let material = ReplayUnit::lib("frust-material", "frust_material");
        let rlibs: BTreeMap<ReplayUnit, PathBuf> = [
            (app.clone(), empty_object.clone()),
            (material.clone(), empty_object.clone()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            gated_inputs(&rlibs, &graph).unwrap(),
            vec![empty_object.clone()],
            "the member's empty rlib is still read (and refused by the gate)"
        );
        let rlibs: BTreeMap<ReplayUnit, PathBuf> =
            [(app, code_object.clone()), (material, code_object.clone())]
                .into_iter()
                .collect();
        assert_eq!(
            gated_inputs(&rlibs, &graph).unwrap(),
            vec![code_object.clone(), code_object]
        );
    }

    #[test]
    fn tip_objects_without_dwarf_are_skipped_unless_none_carries_any() {
        let dir = temp_dir("typed-objects");
        let plain = dir.join("app-1.app.a-cgu.0.rcgu.o");
        std::fs::write(&plain, object(target(), &[Def::Text("main", 8)])).unwrap();
        assert_eq!(
            typed_objects(&[], &["app".into()]).unwrap().paths,
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

    #[test]
    fn the_ungated_ledger_keeps_each_units_latest_objects_until_accepted() {
        let mut ledger = Ungated::default();
        let (a, tip) = (ReplayUnit::lib("a", "a"), ReplayUnit::bin("b", "b"));
        ledger.compiled(a.clone(), vec!["/t/liba-1.rlib".into()]);
        ledger.compiled(
            tip.clone(),
            vec!["/t/b.0.rcgu.o".into(), "/t/b.1.rcgu.o".into()],
        );
        ledger.compiled(a.clone(), vec!["/t/liba-2.rlib".into()]);
        assert!(ledger.contains(&a) && ledger.contains(&tip));
        let paths = |list: &[&str]| list.iter().map(PathBuf::from).collect::<Vec<_>>();
        assert_eq!(
            ledger.inputs(),
            paths(&["/t/liba-2.rlib", "/t/b.0.rcgu.o", "/t/b.1.rcgu.o"])
        );
        ledger.accept();
        assert!(ledger.inputs().is_empty());
        assert!(!ledger.contains(&a));
    }

    /// The real [`DesktopBuilder::compile`] over a scripted rustc whose
    /// rlibs are fixture builds with real DWARF. Not on Windows, like the
    /// fixture itself.
    #[cfg(not(windows))]
    mod ungated_fixtures {
        use super::super::super::layout::fixture;
        use super::super::super::link_intercept::ENV_ARGS_FILE;
        use super::*;

        /// What one replayed rustc answers.
        enum Reply {
            /// A lib compile reporting this rlib, as rustc reports it.
            Rlib(PathBuf),
            /// A tip-bin compile whose intercepted link captured no object.
            Linked,
            /// A compile error with this rendered diagnostic.
            Fails(&'static str),
        }

        /// Answers each replay by its `--crate-name`, in order, and records
        /// the crate names it replayed.
        struct ReplayScript {
            replies: Mutex<HashMap<String, VecDeque<Reply>>>,
            replayed: Mutex<Vec<String>>,
        }

        impl ReplayScript {
            fn new(replies: Vec<(&str, Vec<Reply>)>) -> Arc<Self> {
                Arc::new(Self {
                    replies: Mutex::new(
                        replies
                            .into_iter()
                            .map(|(name, list)| (name.to_string(), list.into()))
                            .collect(),
                    ),
                    replayed: Mutex::new(Vec::new()),
                })
            }

            fn replayed(&self) -> Vec<String> {
                self.replayed.lock().unwrap().clone()
            }
        }

        impl ProcessRunner for ReplayScript {
            fn run(&self, cmd: &str, _args: &[&str]) -> anyhow::Result<Output> {
                anyhow::bail!("unexpected run of `{cmd}`")
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
                _cmd: &str,
                args: &[&str],
                _cwd: Option<&Path>,
                env: &[(&str, &str)],
                _remove_env: &[&str],
                _on_line: &mut dyn FnMut(&str),
            ) -> anyhow::Result<Output> {
                let name = args
                    .windows(2)
                    .find(|pair| pair[0] == "--crate-name")
                    .map(|pair| pair[1].to_string())
                    .expect("a replay names its crate");
                self.replayed.lock().unwrap().push(name.clone());
                let reply = self
                    .replies
                    .lock()
                    .unwrap()
                    .get_mut(&name)
                    .and_then(VecDeque::pop_front)
                    .ok_or_else(|| anyhow::anyhow!("unscripted replay of `{name}`"))?;
                let ok = |stderr: String| Output {
                    success: true,
                    stdout: String::new(),
                    stderr,
                };
                Ok(match reply {
                    Reply::Rlib(rlib) => ok(serde_json::json!({
                        "$message_type": "artifact", "artifact": rlib, "emit": "link"
                    })
                    .to_string()),
                    Reply::Linked => {
                        let (_, file) = env
                            .iter()
                            .find(|(key, _)| *key == ENV_ARGS_FILE)
                            .expect("the tip replay names its link-args file");
                        std::fs::write(file, "[]")?;
                        ok(String::new())
                    }
                    Reply::Fails(rendered) => Output {
                        success: false,
                        stdout: String::new(),
                        stderr: serde_json::json!({
                            "$message_type": "diagnostic", "rendered": rendered
                        })
                        .to_string(),
                    },
                })
            }

            fn spawn_streaming(
                &self,
                cmd: &str,
                _args: &[&str],
                _cwd: Option<&Path>,
                _env: &[(&str, &str)],
            ) -> anyhow::Result<StreamHandle> {
                anyhow::bail!("unexpected spawn of `{cmd}`")
            }
        }

        fn lib_a() -> ReplayUnit {
            ReplayUnit::lib("a", "a")
        }

        fn lib_b() -> ReplayUnit {
            ReplayUnit::lib("b", "b")
        }

        fn tip_bin() -> ReplayUnit {
            ReplayUnit::bin("b", "b-app")
        }

        /// A workspace at `/w`: the tip `b` (lib and bin) depends on `a`.
        fn graph() -> WorkspaceGraph {
            const A: &str = "path+file:///w/a#0.1.0";
            const B: &str = "path+file:///w/b#0.1.0";
            let target = |name: &str, kind: &str, src: &str| serde_json::json!({"name": name, "kind": [kind], "crate_types": [kind], "src_path": src});
            let json = serde_json::json!({
                "packages": [
                    {"id": A, "name": "a", "source": null, "manifest_path": "/w/a/Cargo.toml",
                     "targets": [target("a", "lib", "/w/a/src/lib.rs")]},
                    {"id": B, "name": "b", "source": null, "manifest_path": "/w/b/Cargo.toml",
                     "targets": [target("b", "lib", "/w/b/src/lib.rs"),
                                 target("b-app", "bin", "/w/b/src/main.rs")]},
                ],
                "workspace_members": [A, B],
                "resolve": {"nodes": [
                    {"id": A, "deps": []},
                    {"id": B, "deps": [{"name": "a", "pkg": A,
                                        "dep_kinds": [{"kind": null, "target": null}]}]},
                ], "root": B},
                "workspace_root": "/w",
            });
            WorkspaceGraph::from_metadata(&json.to_string(), "b", None).unwrap()
        }

        fn record(crate_name: &str, crate_type: &str) -> RustcRecord {
            RustcRecord {
                args: [
                    "rustc",
                    "--crate-name",
                    crate_name,
                    "--crate-type",
                    crate_type,
                    "--edition=2024",
                ]
                .map(str::to_string)
                .to_vec(),
                envs: Vec::new(),
                crate_types: vec![crate_type.to_string()],
            }
        }

        /// The fixture has one replayable crate, so each unit's rlib is one
        /// of its builds: `a` starts as `badge-p1` (`Badge { n }`) and `b`
        /// as the no-edit build, whose DWARF has no `Badge` at all — as a
        /// dependent's own objects would not describe a type only `a` uses.
        struct Setup {
            builder: DesktopBuilder,
            accepted: AcceptedSets,
            script: Arc<ReplayScript>,
            b_rlib: PathBuf,
        }

        fn setup(tag: &str, replies: Vec<(&str, Vec<Reply>)>) -> Setup {
            let a_base = fixture::edited("badge-p1");
            let b_rlib = fixture::base();
            let crates = fixture::crates();
            let dir = temp_dir(tag);
            let base = [a_base.clone(), b_rlib.clone()];
            let accepted = AcceptedSets::begin(
                &dir,
                "session-b",
                layout::extract(&base, &crates).unwrap().table,
                SeamSet::from_inputs(&base).unwrap(),
            )
            .unwrap();
            let script = ReplayScript::new(replies);
            let image = object(target(), &[Def::Text(ANCHOR_SYMBOL, 4)]);
            let builder = DesktopBuilder {
                runner: Arc::clone(&script) as Arc<dyn ProcessRunner + Send + Sync>,
                graph: graph(),
                records: [
                    (lib_a(), record("a", "lib")),
                    (lib_b(), record("b", "lib")),
                    (tip_bin(), record("b_app", "bin")),
                ]
                .into_iter()
                .map(|(unit, record)| (unit.record_key(), record))
                .collect(),
                modified: super::super::super::graph::ModifiedSet::new(),
                dirty: BTreeSet::new(),
                ungated: Ungated::default(),
                rlibs: [(lib_a(), a_base), (lib_b(), b_rlib.clone())]
                    .into_iter()
                    .collect(),
                tip_link_args: Vec::new(),
                tip_env: Vec::new(),
                image_unit: tip_bin(),
                cache: SymbolCache::from_bytes("base", &image, target()).unwrap(),
                target: target(),
                flavor: LinkerFlavor::for_triple(TRIPLE).unwrap(),
                linker: "cc".to_string(),
                upload_strip: None,
                target_dir: dir.clone(),
                session: "session-b".to_string(),
                session_dir: accepted.dir().to_path_buf(),
                scope_dir: dir,
                frust_exe: PathBuf::from("/opt/frust/bin/frust"),
                crates,
                tip_replays: 0,
                linked_layouts: None,
                timings: PatchTimings::default(),
                accepted_objects: HashSet::new(),
                tip_digests: Vec::new(),
                image_reads: Vec::new(),
                seam_names: HashMap::new(),
            };
            Setup {
                builder,
                accepted,
                script,
                b_rlib,
            }
        }

        fn units(list: &[ReplayUnit]) -> BTreeSet<ReplayUnit> {
            list.iter().cloned().collect()
        }

        fn failed(compiled: Result<Compiled, HotpatchError>) -> Vec<String> {
            match compiled {
                Ok(Compiled::Failed { diagnostics }) => diagnostics,
                Ok(Compiled::Candidate { .. }) => {
                    panic!("expected a failed round, got a candidate")
                }
                Ok(Compiled::Nothing) => panic!("expected a failed round, got nothing"),
                Err(err) => panic!("expected a failed round, got {err:?}"),
            }
        }

        fn candidate(compiled: Result<Compiled, HotpatchError>) -> (LayoutTable, SeamSet) {
            match compiled {
                Ok(Compiled::Candidate { layouts, seams }) => (layouts, seams),
                Ok(Compiled::Failed { diagnostics }) => {
                    panic!("expected a candidate, got a failed round: {diagnostics:?}")
                }
                Ok(Compiled::Nothing) => panic!("expected a candidate, got nothing"),
                Err(err) => panic!("expected a candidate, got {err:?}"),
            }
        }

        fn badge_grew() -> RestartReason {
            RestartReason::LayoutChanged {
                records: vec![format!(
                    "{}::Badge changed layout (4 → 8 bytes)",
                    fixture::APP_CRATE
                )],
            }
        }

        /// The review's counterexample: `a` grows `Badge` and compiles, its
        /// dependent `b` fails in the same round; the user fixes `b` alone.
        /// The patch would link `a`'s new rlib, so the candidate must carry
        /// its DWARF and be refused.
        #[test]
        fn a_compiles_b_fails_then_fixing_b_refuses_a_changed_type() {
            let a_new = fixture::edited("badge-p2");
            let Setup {
                mut builder,
                accepted,
                script,
                b_rlib,
            } = setup(
                "ungated-lib",
                vec![
                    ("a", vec![Reply::Rlib(a_new.clone())]),
                    (
                        "b",
                        vec![
                            Reply::Fails("error[E0308]: mismatched types\n"),
                            Reply::Rlib(fixture::base()),
                        ],
                    ),
                ],
            );

            // Round N: `a` compiles, `b` fails.
            assert_eq!(
                failed(builder.compile(&units(&[lib_a()]))),
                vec!["error[E0308]: mismatched types\n".to_string()]
            );
            assert_eq!(builder.rlibs[&lib_a()], a_new);
            assert!(!builder.dirty.contains(&lib_a()));
            assert!(builder.dirty.contains(&lib_b()));
            assert!(builder.ungated.contains(&lib_a()), "`a` is still ungated");
            assert!(!builder.ungated.contains(&lib_b()));

            // Round N+1: only `b` replays, yet `a`'s objects are extracted.
            let (layouts, seams) = candidate(builder.compile(&units(&[lib_b()])));
            assert_eq!(script.replayed(), vec!["a", "b", "b"]);
            assert_eq!(accepted.check(&layouts, seams.clone()), Err(badge_grew()));
            assert_eq!(
                builder.ungated.inputs(),
                vec![a_new.clone(), b_rlib.clone()]
            );
            assert_eq!(
                builder.modified_rlibs().unwrap(),
                vec![b_rlib.clone(), a_new],
                "the patch would link `a`'s new rlib"
            );

            // Negative control: `b`'s objects alone, the table the gate read
            // before the ledger, pass — they could never have caught it.
            let b_only = layout::extract(&[b_rlib], &fixture::crates())
                .unwrap()
                .table;
            assert!(accepted.check(&b_only, seams).is_ok());
        }

        /// The same through the tip bin: `a` compiles, the tip bin fails;
        /// the user fixes `main.rs` alone, whose objects describe nothing
        /// of `a`.
        #[test]
        fn a_compiles_the_tip_fails_then_fixing_the_tip_refuses_a_changed_type() {
            let a_new = fixture::edited("badge-p2");
            let Setup {
                mut builder,
                accepted,
                script,
                b_rlib,
            } = setup(
                "ungated-tip",
                vec![
                    ("a", vec![Reply::Rlib(a_new.clone())]),
                    ("b", vec![Reply::Rlib(fixture::base())]),
                    (
                        "b_app",
                        vec![Reply::Fails("error[E0425]: x\n"), Reply::Linked],
                    ),
                ],
            );

            failed(builder.compile(&units(&[lib_a(), tip_bin()])));
            assert!(builder.dirty.contains(&tip_bin()));
            assert!(builder.ungated.contains(&lib_a()));
            assert!(builder.ungated.contains(&lib_b()));

            let (layouts, seams) = candidate(builder.compile(&units(&[tip_bin()])));
            assert_eq!(script.replayed(), vec!["a", "b", "b_app", "b_app"]);
            assert!(builder.dirty.is_empty());
            assert_eq!(accepted.check(&layouts, seams), Err(badge_grew()));
            assert_eq!(builder.ungated.inputs(), vec![a_new, b_rlib]);
        }

        /// Android's image is the tip lib's `cdylib`: the builder replays
        /// the tip lib with its link intercepted (its objects are the image
        /// link line), links a dependency's rlib but never the tip lib's
        /// own, gates every object it compiled, and classifies a file only a
        /// bin compiles as nothing the image was built from. With the tip
        /// bin as the image (desktop), the bin's root still replays.
        #[test]
        fn a_lib_image_replays_the_tip_lib_as_the_image_and_drops_bins() {
            let a_new = fixture::edited("badge-p2");
            let Setup {
                mut builder,
                accepted,
                script,
                ..
            } = setup(
                "lib-image",
                vec![
                    ("a", vec![Reply::Rlib(a_new.clone())]),
                    ("b", vec![Reply::Linked]),
                ],
            );
            let main_rs = Path::new("/w/b/src/main.rs");
            assert_eq!(
                builder.classify(main_rs),
                PathClass::Replayable {
                    units: units(&[tip_bin()])
                }
            );
            builder.image_unit = lib_b();
            assert_eq!(builder.classify(main_rs), PathClass::Unaffected);
            assert_eq!(
                builder.classify(Path::new("/w/b/src/lib.rs")),
                PathClass::Replayable {
                    units: units(&[lib_b()])
                }
            );

            let (layouts, seams) = candidate(builder.compile(&units(&[lib_a()])));
            assert_eq!(script.replayed(), vec!["a", "b"]);
            assert!(builder.dirty.is_empty());
            assert!(builder.ungated.contains(&lib_b()));
            assert_eq!(builder.modified_rlibs().unwrap(), vec![a_new.clone()]);
            assert_eq!(builder.patch_inputs().unwrap(), vec![a_new.clone()]);
            assert_eq!(accepted.check(&layouts, seams), Err(badge_grew()));
            assert_eq!(builder.ungated.inputs(), vec![a_new]);
        }

        /// An rlib like a platform shell compiled for another target: crate
        /// metadata and one object holding neither code nor data.
        fn code_less_rlib(dir: &Path) -> PathBuf {
            let empty = super::super::super::link_intercept::empty_object(
                object::BinaryFormat::MachO,
                object::Architecture::Aarch64,
            )
            .unwrap();
            let rlib = dir.join("libshell-0.rlib");
            let mut builder = ar::Builder::new(std::fs::File::create(&rlib).unwrap());
            for (name, bytes) in [
                ("lib.rmeta", b"rust".to_vec()),
                ("shell-0.shell.0-cgu.0.rcgu.o", empty),
            ] {
                builder
                    .append(
                        &ar::Header::new(name.as_bytes().to_vec(), bytes.len() as u64),
                        bytes.as_slice(),
                    )
                    .unwrap();
            }
            rlib
        }

        /// A workspace at `/w` whose tip `b` (lib and bin) depends on the
        /// non-members `a` and `shell` under `/x`, `shell` on `a` too, both
        /// captured: an edit to `a` cascades through `shell`.
        fn graph_through_a_shell() -> WorkspaceGraph {
            const A: &str = "path+file:///x/a#0.1.0";
            const SHELL: &str = "path+file:///x/shell#0.1.0";
            const B: &str = "path+file:///w/b#0.1.0";
            let target = |name: &str, kind: &str, src: &str| serde_json::json!({"name": name, "kind": [kind], "crate_types": [kind], "src_path": src});
            let normal = |pkg: &str| serde_json::json!({"name": pkg, "pkg": pkg, "dep_kinds": [{"kind": null, "target": null}]});
            let json = serde_json::json!({
                "packages": [
                    {"id": A, "name": "a", "source": null, "manifest_path": "/x/a/Cargo.toml",
                     "targets": [target("a", "lib", "/x/a/src/lib.rs")]},
                    {"id": SHELL, "name": "shell", "source": null,
                     "manifest_path": "/x/shell/Cargo.toml",
                     "targets": [target("shell", "lib", "/x/shell/src/lib.rs")]},
                    {"id": B, "name": "b", "source": null, "manifest_path": "/w/b/Cargo.toml",
                     "targets": [target("b", "lib", "/w/b/src/lib.rs"),
                                 target("b-app", "bin", "/w/b/src/main.rs")]},
                ],
                "workspace_members": [B],
                "resolve": {"nodes": [
                    {"id": A, "deps": []},
                    {"id": SHELL, "deps": [normal(A)]},
                    {"id": B, "deps": [normal(A), normal(SHELL)]},
                ], "root": B},
                "workspace_root": "/w",
            });
            let mut graph = WorkspaceGraph::from_metadata(&json.to_string(), "b", None).unwrap();
            let records = [
                RecordKey::parse("a.lib").unwrap(),
                RecordKey::parse("shell.lib").unwrap(),
                RecordKey::parse("b.lib").unwrap(),
                RecordKey::parse("b_app.bin").unwrap(),
            ]
            .into_iter()
            .map(|key| {
                let record = record(&key.crate_name, key.kind.suffix());
                (key, record)
            })
            .collect();
            assert_eq!(graph.replay_non_members(&records), vec!["a", "shell"]);
            graph
        }

        /// The base table leaves a captured non-member's code-less rlib out
        /// ([`gated_inputs`]); a cascade that replays it must leave it out
        /// of the candidate's table too, or every edit below it is refused.
        #[test]
        fn a_cascade_through_a_code_less_path_dependency_passes_the_candidate_gate() {
            let a_rlib = fixture::edited("badge-p1");
            let Setup {
                mut builder,
                accepted,
                script,
                b_rlib,
            } = setup(
                "code-less-shell",
                vec![
                    ("a", vec![Reply::Rlib(a_rlib.clone())]),
                    ("b", vec![Reply::Rlib(fixture::base())]),
                ],
            );
            let shell_rlib = code_less_rlib(&builder.scope_dir);
            script
                .replies
                .lock()
                .unwrap()
                .insert("shell".into(), [Reply::Rlib(shell_rlib.clone())].into());
            let shell = ReplayUnit::lib("shell", "shell");
            builder.graph = graph_through_a_shell();
            builder
                .records
                .insert(shell.record_key(), record("shell", "lib"));
            builder.rlibs.insert(shell.clone(), shell_rlib.clone());
            assert_eq!(
                gated_inputs(&builder.rlibs, &builder.graph).unwrap(),
                vec![a_rlib.clone(), b_rlib.clone()],
                "the base table's inputs"
            );

            let (layouts, seams) = candidate(builder.compile(&units(&[lib_a()])));
            assert_eq!(script.replayed(), vec!["a", "shell", "b"]);
            assert!(builder.ungated.contains(&shell));
            let checked = accepted.check(&layouts, seams);
            assert!(checked.is_ok(), "{checked:?}");
            assert_eq!(
                builder.modified_rlibs().unwrap(),
                vec![b_rlib.clone(), shell_rlib.clone(), a_rlib.clone()],
                "the shell's rlib is still linked into the patch"
            );

            // Negative control: the candidate's inputs unfiltered, as the
            // builder read them before, are refused outright.
            let unfiltered = builder.ungated.inputs();
            assert_eq!(unfiltered, vec![a_rlib, b_rlib, shell_rlib]);
            let refused = layout::extract(&unfiltered, &fixture::crates());
            assert!(
                matches!(refused, Err(HotpatchError::BuilderUnsupported { .. })),
                "{refused:?}"
            );
        }

        /// Accepting a candidate empties the set: the next candidate is
        /// extracted from its own round's objects only.
        #[test]
        fn the_cached_seam_set_equals_a_full_read_and_is_kept_per_content() {
            let a_rlib = fixture::edited("badge-p1");
            let Setup { mut builder, .. } = setup(
                "seam-cache",
                vec![
                    ("a", vec![Reply::Rlib(a_rlib)]),
                    ("b", vec![Reply::Rlib(fixture::base())]),
                ],
            );
            let (_, seams) = candidate(builder.compile(&units(&[lib_a()])));
            let inputs = builder.patch_inputs().unwrap();
            assert_eq!(seams, SeamSet::from_inputs(&inputs).unwrap());
            assert!(!seams.is_empty(), "the fixture carries a seam");
            let digests = object_digests(&inputs).unwrap();
            assert!(digests.iter().all(|d| builder.seam_names.contains_key(d)));
            assert_eq!(
                builder.seam_set().unwrap(),
                seams,
                "a cached read answers the same"
            );
        }

        #[test]
        fn seam_candidates_keep_exactly_what_the_seam_reader_does_not_skip() {
            let base = fixture::base();
            let names = seam_candidates(&base).unwrap();
            assert!(!names.is_empty());
            assert_eq!(
                SeamSet::from_symbols(names.iter().map(String::as_str)).unwrap(),
                SeamSet::from_inputs(&[base]).unwrap()
            );
        }

        #[test]
        fn a_tip_object_whose_bytes_were_accepted_is_not_read_again() {
            let Setup { mut builder, .. } = setup("accepted-objects", Vec::new());
            let dir = temp_dir("accepted-objects-files");
            let junk = dir.join("b_app-1.b_app.a-cgu.0.rcgu.o");
            std::fs::write(&junk, b"not an object").unwrap();
            let junk = vec![junk];
            // New bytes are read, and these fail closed.
            assert!(builder.fresh_typed_objects(&junk).is_err());
            let digest = object_digests(&junk).unwrap()[0];
            builder.accepted_objects.insert(digest);
            assert_eq!(
                builder.fresh_typed_objects(&junk).unwrap(),
                Vec::<PathBuf>::new()
            );
            assert!(builder.image_reads.is_empty());
            assert_eq!(builder.tip_digests, vec![digest]);
        }

        #[test]
        fn an_accepted_candidates_tip_objects_join_the_accepted_bytes() {
            let Setup { mut builder, .. } = setup("accepted-join", Vec::new());
            let dir = temp_dir("accepted-join-files");
            let seen = dir.join("b_app-1.b_app.a-cgu.0.rcgu.o");
            std::fs::write(&seen, b"bytes gated before").unwrap();
            let plain = dir.join("b_app-1.b_app.a-cgu.1.rcgu.o");
            std::fs::write(&plain, object(target(), &[Def::Text("main", 8)])).unwrap();
            let digests = object_digests(&[seen.clone(), plain.clone()]).unwrap();
            // No object of a round carries DWARF and none was skipped: the
            // build's debug level is suspect, and nothing is recorded.
            assert!(
                builder
                    .fresh_typed_objects(std::slice::from_ref(&plain))
                    .is_err()
            );
            assert!(builder.tip_digests.is_empty());
            // Beside a skipped object, a DWARF-less one is merely untyped.
            builder.accepted_objects.insert(digests[0]);
            let both = [seen, plain.clone()];
            assert_eq!(
                builder.fresh_typed_objects(&both).unwrap(),
                Vec::<PathBuf>::new()
            );
            assert_eq!(builder.tip_digests, digests);
            assert!(!builder.accepted_objects.contains(&digests[1]));
            builder.accepted();
            assert!(builder.accepted_objects.contains(&digests[1]));
            assert_eq!(
                builder
                    .fresh_typed_objects(std::slice::from_ref(&plain))
                    .unwrap(),
                Vec::<PathBuf>::new(),
                "accepted bytes are skipped"
            );
        }

        #[test]
        fn the_acceptance_hook_empties_the_builders_ungated_set() {
            let a_rlib = fixture::edited("badge-p1");
            let Setup {
                mut builder,
                script,
                b_rlib,
                ..
            } = setup(
                "ungated-accept",
                vec![
                    ("a", vec![Reply::Rlib(a_rlib.clone())]),
                    (
                        "b",
                        vec![Reply::Rlib(fixture::base()), Reply::Rlib(fixture::base())],
                    ),
                ],
            );
            candidate(builder.compile(&units(&[lib_a()])));
            assert_eq!(builder.ungated.inputs(), vec![a_rlib, b_rlib.clone()]);
            builder.accepted();
            assert!(builder.ungated.inputs().is_empty());

            let (layouts, _) = candidate(builder.compile(&units(&[lib_b()])));
            assert_eq!(script.replayed(), vec!["a", "b", "b"]);
            assert_eq!(builder.ungated.inputs(), vec![b_rlib.clone()]);
            assert_eq!(
                layouts,
                layout::extract(&[b_rlib], &fixture::crates())
                    .unwrap()
                    .table
            );
        }
    }

    /// The cancellable discovery wait observes a raised `cancel` at its
    /// next poll and leaves the child to the caller. Deterministic: the
    /// cancel is raised by the line the fake sends, so the wait forwards
    /// that line and then ends on the flag (not the deadline, which would
    /// answer `Failure`).
    #[test]
    fn the_discovery_wait_ends_when_cancel_is_raised() {
        let runner = crate::process::FakeProcessRunner::new()
            .with_hanging_stream("app", ["I/app: starting"]);
        let mut child = runner.spawn_streaming("app", &[], None, &[]).unwrap();
        let cancel = AtomicBool::new(false);
        let mut lines = Vec::new();
        let announced = read_discovery_cancellable(
            &mut child,
            &mut |line| {
                lines.push(line.to_string());
                cancel.store(true, Ordering::SeqCst);
            },
            &cancel,
        );
        assert!(matches!(announced, Err(Cancelled)));
        assert_eq!(lines, vec!["I/app: starting"]);
        child.kill();
    }
}

/// The Windows session path over real images: the hot-patch fixture
/// workspace (with a bin added to its app package) compiled by the pinned
/// rustc, its captured invocations written as capture records, the fat exe
/// linked by [`link_base`] and one change replayed and thin-linked by the
/// real builder into `patch-1.dll`, each table read from a PDB. Everything
/// lives under the test binary's own target dir, where built binaries run
/// in place.
#[cfg(all(test, windows))]
mod windows {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::super::capture::{RecordKey, RustcRecord, write_record};
    use super::super::pe;
    use super::super::symbols::ANCHOR_SYMBOL;
    use super::*;
    use crate::process::RealProcessRunner;

    const APP_PACKAGE: &str = "hotpatch-fixture-app";
    const APP_CRATE: &str = "hotpatch_fixture_app";

    /// The bin added to the fixture's app package: it reaches the app's
    /// entry points (so a patch links their objects) and prints its anchor's
    /// runtime address. `mount_home_by_value` it never names.
    const BIN_SOURCE: &str = r#"
#[unsafe(no_mangle)]
pub extern "C" fn __frust_hotpatch_anchor() {}

fn main() {
    let home = hotpatch_fixture_app::mount_home();
    let counter = hotpatch_fixture_app::mount_counter();
    let press = hotpatch_fixture_app::on_press(1);
    std::hint::black_box((&home, &counter, &press));
    println!("{}", __frust_hotpatch_anchor as usize);
}
"#;

    fn triple() -> &'static str {
        if cfg!(target_arch = "aarch64") {
            "aarch64-pc-windows-msvc"
        } else {
            "x86_64-pc-windows-msvc"
        }
    }

    fn rustc() -> String {
        let cargo = option_env!("CARGO").unwrap_or("cargo");
        let sibling = Path::new(cargo).with_file_name("rustc.exe");
        if sibling.is_file() {
            sibling.display().to_string()
        } else {
            "rustc".to_string()
        }
    }

    /// A fresh directory beside the test binary's `deps`.
    fn fixture_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap().parent().unwrap().join(format!(
            "frust-hotpatch-session-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if name == "target" || name == "Cargo.lock" {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &to.join(&name));
            } else {
                std::fs::copy(entry.path(), to.join(&name)).unwrap();
            }
        }
    }

    /// The arguments of the command `rustc --print link-args` printed (Rust
    /// `Debug` strings separated by spaces), linker program dropped.
    fn parse_link_args(printed: &str) -> Vec<String> {
        let line = printed
            .lines()
            .find(|line| line.starts_with('"'))
            .unwrap_or_else(|| panic!("no link line in {printed:?}"));
        let mut args = Vec::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '"' {
                continue;
            }
            let mut arg = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => arg.push(chars.next().unwrap()),
                    '"' => break,
                    c => arg.push(c),
                }
            }
            args.push(arg);
        }
        args.remove(0);
        args
    }

    /// One fixture compile: `crate_type` `crate_name` from `src` (relative
    /// to the workspace root, where cargo runs rustc too), written to `deps`
    /// as `-<extra>`, recorded into `scope`. Returns rustc's stdout.
    #[allow(clippy::too_many_arguments)]
    fn compile(
        ws: &Path,
        deps: &Path,
        scope: &Path,
        debuginfo: &str,
        crate_name: &str,
        crate_type: &str,
        src: &str,
        extra: &str,
        externs: &[(&str, &str)],
        tip_flags: &[&str],
    ) -> String {
        let deps_text = deps.display().to_string();
        let mut args: Vec<String> = [
            "--crate-name",
            crate_name,
            "--edition=2024",
            src,
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--crate-type",
            crate_type,
            "--emit=dep-info,link",
            "-C",
            &format!("debuginfo={debuginfo}"),
            "-C",
            &format!("metadata={extra}"),
            "-C",
            &format!("extra-filename=-{extra}"),
            "--out-dir",
            &deps_text,
            "-L",
            &format!("dependency={deps_text}"),
        ]
        .map(str::to_string)
        .to_vec();
        for (name, extra) in externs {
            args.push("--extern".to_string());
            args.push(format!(
                "{name}={}",
                deps.join(format!("lib{name}-{extra}.rlib")).display()
            ));
        }
        args.extend(tip_flags.iter().map(|flag| flag.to_string()));
        let rustc = rustc();
        let mut recorded = vec![rustc.clone()];
        recorded.extend(args.iter().cloned());
        let crate_types = vec![crate_type.to_string()];
        write_record(
            scope,
            &RecordKey::new(crate_name, &crate_types),
            &RustcRecord {
                args: recorded,
                envs: Vec::new(),
                crate_types,
            },
        )
        .unwrap();
        if crate_type == "bin" {
            args.extend(["--print", "link-args"].map(str::to_string));
        }
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = RealProcessRunner
            .run_streaming(&rustc, &argv, Some(ws), &[], &mut |_| {})
            .unwrap();
        assert!(
            out.success,
            "rustc {crate_name} ({crate_type}): {}",
            out.stderr
        );
        out.stdout
    }

    /// The fixture's fat exe, linked by [`link_base`], and the app source a
    /// test edits.
    struct Fat {
        base: FatBase,
        app_source: PathBuf,
        link_args: Vec<String>,
        /// The fixture's directory, removed with it.
        dir: PathBuf,
    }

    impl Drop for Fat {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl Fat {
        fn builder(&mut self) -> &mut DesktopBuilder {
            &mut self.base.builder
        }

        /// Saves an edit to the app crate: the fixture `feature` turned on.
        fn edit(&self, feature: &str) {
            let source = std::fs::read_to_string(&self.app_source).unwrap();
            let edited = source.replace(&format!("feature = \"{feature}\""), "all()");
            assert_ne!(source, edited, "{feature} is a fixture edit");
            std::fs::write(&self.app_source, edited).unwrap();
        }

        /// Runs the fat exe in place: the anchor's runtime address.
        fn anchor_runtime(&self) -> u64 {
            let run = std::process::Command::new(self.base.image())
                .output()
                .unwrap();
            assert!(run.status.success(), "{run:?}");
            String::from_utf8(run.stdout)
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        }
    }

    /// Compiles the fixture (`debuginfo` for every crate) and links its fat
    /// exe through [`link_base`].
    fn fat(tag: &str, debuginfo: &str) -> Result<Fat, HotpatchError> {
        let dir = fixture_dir(tag);
        let ws = dir.join("ws");
        copy_tree(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hotpatch"),
            &ws,
        );
        std::fs::write(ws.join("app/src/main.rs"), BIN_SOURCE).unwrap();
        let target_dir = dir.join("target");
        let deps = target_dir.join("debug").join("deps");
        std::fs::create_dir_all(&deps).unwrap();
        let scope = dir.join("scope");
        let build = |name, ty, src, extra, externs: &[(&str, &str)], tip: &[&str]| {
            compile(
                &ws, &deps, &scope, debuginfo, name, ty, src, extra, externs, tip,
            )
        };
        build("frust_hotpatch", "rlib", "hotfn/src/lib.rs", "f1", &[], &[]);
        build(
            "frust_core",
            "rlib",
            "core/src/lib.rs",
            "c1",
            &[("frust_hotpatch", "f1")],
            &[],
        );
        build(
            APP_CRATE,
            "rlib",
            "app/src/lib.rs",
            "a1",
            &[("frust_core", "c1")],
            &[],
        );
        let printed = build(
            APP_CRATE,
            "bin",
            "app/src/main.rs",
            "b1",
            &[(APP_CRATE, "a1")],
            &["-Csave-temps=true", "-Clink-dead-code"],
        );
        let link_args = parse_link_args(&printed);

        let runner = RealProcessRunner;
        let metadata =
            super::super::graph::cargo_metadata(&runner, &ws.join("Cargo.toml"), None).unwrap();
        let graph = WorkspaceGraph::from_metadata(&metadata, APP_PACKAGE, None).unwrap();
        let tip_bin = graph.tip_bin();
        let fat_dir = target_dir.join("frust-hotpatch").join(FAT_DIR).join("test");
        std::fs::create_dir_all(&fat_dir).unwrap();
        let host = SessionHost {
            runner: Arc::new(RealProcessRunner),
            env: &RealEnv,
            frust_exe: PathBuf::from("frust.exe"),
        };
        let started = Instant::now();
        let base = link_base(
            &host,
            BaseRequest {
                graph,
                link_args: link_args.clone(),
                image_unit: tip_bin.clone(),
                image: fat_dir.join(desktop_image_name(LinkerFlavor::Msvc, &tip_bin.target)),
                custom_linker: None,
                upload_strip: None,
                target: Target::from_triple(triple()).unwrap(),
                flavor: LinkerFlavor::Msvc,
                target_dir,
                archive_dir: &fat_dir,
                scope_dir: scope,
                session: desktop_session_name(&tip_bin.target),
            },
        )
        .inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&dir);
        })?;
        eprintln!(
            "{tag}: link_base (fat link, PDB cache, PDB base table) took {:?} with `{}`",
            started.elapsed(),
            base.builder.linker
        );
        Ok(Fat {
            base,
            app_source: ws.join("app/src/lib.rs"),
            link_args,
            dir,
        })
    }

    fn app_lib() -> BTreeSet<ReplayUnit> {
        [ReplayUnit::lib(APP_PACKAGE, APP_CRATE)].into()
    }

    fn app(path: &str) -> String {
        format!("{APP_CRATE}::{path}")
    }

    /// `image`'s preferred base from its headers and its PDB's anchor RVA.
    fn base_and_anchor_rva(image: &Path) -> (u64, u64) {
        let base = pe::image_base(&std::fs::read(image).unwrap(), "fixture").unwrap();
        let pdb = pe::read_pdb(image).unwrap();
        let rva = pdb
            .records
            .iter()
            .find(|r| {
                r.name == ANCHOR_SYMBOL && r.kind == pe::RecordKind::Public { function: true }
            })
            .and_then(|r| r.rva)
            .expect("the anchor's RVA");
        (base, u64::from(rva))
    }

    /// `IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE`: the image may be relocated
    /// by ASLR.
    const DYNAMIC_BASE: u16 = 0x0040;

    /// The PE32+ optional header's `DllCharacteristics` of `image`.
    fn dll_characteristics(image: &Path) -> u16 {
        let bytes = std::fs::read(image).unwrap();
        let at = |offset: usize, len: usize| &bytes[offset..offset + len];
        let pe = u32::from_le_bytes(at(0x3c, 4).try_into().unwrap()) as usize;
        assert_eq!(at(pe, 4), b"PE\0\0");
        let optional = pe + 4 + 20;
        assert_eq!(at(optional, 2), 0x20bu16.to_le_bytes(), "a PE32+ image");
        u16::from_le_bytes(at(optional + 70, 2).try_into().unwrap())
    }

    /// Loads `dll` (which exports the anchor) into a fresh process: a small
    /// exe linked against the DLL's import library, beside it, that prints
    /// the anchor's runtime address in the loaded DLL.
    fn anchor_runtime_when_loaded(dll: &Path) -> u64 {
        let dir = dll.parent().unwrap();
        let stem = dll.file_stem().unwrap().to_str().unwrap();
        assert!(
            dir.join(format!("{stem}.lib")).is_file(),
            "lld-link wrote the import library"
        );
        let source = dir.join("load-patch.rs");
        // Edition 2021: a plain extern block; taking the address calls
        // nothing.
        std::fs::write(
            &source,
            format!(
                "#[link(name = \"{stem}\")]\nextern \"C\" {{\n    fn __frust_hotpatch_anchor();\n}}\n\
                 fn main() {{\n    println!(\"{{}}\", __frust_hotpatch_anchor as usize);\n}}\n"
            ),
        )
        .unwrap();
        let loader = dir.join("load-patch.exe");
        let dir_text = dir.display().to_string();
        let out = RealProcessRunner
            .run(
                &rustc(),
                &[
                    "--edition=2021",
                    "-L",
                    &format!("native={dir_text}"),
                    "-o",
                    &loader.display().to_string(),
                    &source.display().to_string(),
                ],
            )
            .unwrap();
        assert!(out.success, "rustc load-patch.rs: {}", out.stderr);
        let run = std::process::Command::new(&loader).output().unwrap();
        assert!(run.status.success(), "{run:?}");
        String::from_utf8(run.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    /// Replays the saved edit and links `patch-1.dll` against the running
    /// fat exe; the candidate's seams and its post-link table.
    fn patch(fat: &mut Fat) -> (LinkedPatch, SeamSet, LayoutTable) {
        let anchor_runtime = fat.anchor_runtime();
        let builder = fat.builder();
        let (layouts, seams) = match builder.compile(&app_lib()).unwrap() {
            Compiled::Candidate { layouts, seams } => (layouts, seams),
            Compiled::Nothing => panic!("nothing compiled"),
            Compiled::Failed { diagnostics } => panic!("{diagnostics:#?}"),
        };
        assert!(layouts.is_empty(), "no table before the link on Msvc");
        let started = Instant::now();
        let linked = builder.link(1, anchor_runtime).unwrap();
        let table = builder.linked_layouts().expect("the patch's PDB table");
        eprintln!(
            "thin link + patch PDB reads took {:?}: {} types",
            started.elapsed(),
            table.len()
        );
        assert!(builder.linked_layouts().is_none(), "taken once");
        (linked, seams, table)
    }

    #[test]
    fn the_fat_exe_takes_the_archive_whole_and_its_pdb_seeds_cache_anchor_and_base_table() {
        let fat = fat("base", "2").unwrap();
        let image = fat.base.image().to_path_buf();
        assert!(image.ends_with("hotpatch-fixture-app.exe"), "{image:?}");
        assert!(pe::pdb_path(&image).is_file());
        assert_eq!(fat.base.symbol_source(), image);

        // `/WHOLEARCHIVE:` named the packed archive, and no packed rlib
        // stayed on the line.
        let archive = std::fs::read_dir(image.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "a"))
            .expect("the fat archive");
        let fat_args = fat_link::fat_link_args(
            LinkerFlavor::Msvc,
            &fat.link_args,
            Some(&fat_link::FatArchive {
                path: archive.clone(),
                kept_rlibs: Vec::new(),
            }),
            &image,
        )
        .unwrap();
        assert!(fat_args.contains(&format!("/WHOLEARCHIVE:{}", fat_link::render(&archive))));
        let linker = fat.base.builder.linker.clone();
        assert!(
            linker.to_ascii_lowercase().ends_with("rust-lld.exe"),
            "{linker}"
        );

        // The cache and the anchor come from the exe's PDB, and the code of
        // a function the bin never names was linked: the archive was taken
        // whole.
        let cache = &fat.base.builder.cache;
        assert_eq!(cache.anchor_address(), pe::anchor_address(&image).unwrap());
        assert!(
            cache
                .symbols()
                .iter()
                .any(|(name, s)| name.contains("mount_home_by_value") && s.is_section_defined()),
            "an unreferenced app function is in the fat exe"
        );

        // The base table is the PDB's.
        let base_table = fat.base.accepted.layouts();
        assert_eq!(base_table.get(&app("HomeState")).map(|e| e.size), Some(4));
        assert_eq!(base_table.get(&app("HomeState")).map(|e| e.align), Some(0));

        // The anchor is a VA, and the fat exe runs loaded at a 64 KiB-aligned
        // base: on x64 its fixed preferred base, so its slide is 0.
        let (image_base, anchor_rva) = base_and_anchor_rva(&image);
        assert_eq!(cache.anchor_address(), image_base + anchor_rva);
        let slide = fat.anchor_runtime().wrapping_sub(cache.anchor_address());
        assert_eq!(slide & 0xffff, 0, "{slide:#x}");
        if cfg!(target_arch = "x86_64") {
            assert_eq!(image_base, fat_link::FAT_IMAGE_BASE, "{image_base:#x}");
            assert_eq!(slide, 0, "the fat exe loaded at its preferred base");
        }
    }

    #[test]
    fn a_layout_preserving_edit_links_patch_1_and_passes_its_pdb_table() {
        let mut fat = fat("pass", "2").unwrap();
        fat.edit("sentinel-bump");
        let (linked, seams, table) = patch(&mut fat);
        let session_dir = fat.base.accepted.dir().to_path_buf();
        assert_eq!(linked.path, session_dir.join("patch-1.dll"));
        assert!(session_dir.join("patch-1.pdb").is_file());
        assert!(
            !linked.table.map.is_empty(),
            "the jump table maps the patch"
        );

        // Both table anchors are VAs (preferred base + anchor RVA), so the
        // runtime's anchor check passes: the exe's implied offset is its
        // slide (0 at the fixed x64 base) and the DLL's is its load base
        // minus its preferred base, 0 at its own fixed x64 base.
        let (exe_base, exe_rva) = base_and_anchor_rva(fat.base.image());
        let (dll_base, dll_rva) = base_and_anchor_rva(&linked.path);
        assert_eq!(linked.table.aslr_reference, exe_base + exe_rva);
        assert_eq!(linked.table.new_base_address, dll_base + dll_rva);
        let implied = fat
            .anchor_runtime()
            .wrapping_sub(linked.table.aslr_reference);
        if cfg!(target_arch = "x86_64") {
            assert_eq!(implied, 0, "base anchor implies offset {implied:#x}");
        } else {
            assert_eq!(implied & 0xffff, 0, "{implied:#x}");
        }
        let characteristics = dll_characteristics(&linked.path);
        eprintln!(
            "patch-1.dll: ImageBase {dll_base:#x}, DllCharacteristics {characteristics:#06x}, \
             anchor RVA {dll_rva:#x}, new_base_address {:#x}",
            linked.table.new_base_address
        );
        if cfg!(target_arch = "x86_64") {
            // Patch 1 links at its fixed base with ASLR off, and loads there:
            // its anchor runs at its VA, the slide the runtime reports (0).
            assert_eq!(Some(dll_base), fat_link::patch_image_base(1));
            assert_eq!(
                characteristics & DYNAMIC_BASE,
                0,
                "DYNAMICBASE is clear: {characteristics:#06x}"
            );
            assert_eq!(
                linked.table.new_base_address,
                fat_link::PATCH_IMAGE_BASE_FIRST + dll_rva
            );
            let loaded = anchor_runtime_when_loaded(&linked.path);
            eprintln!("patch-1.dll loaded: anchor at {loaded:#x}");
            assert_eq!(
                loaded.wrapping_sub(linked.table.new_base_address),
                0,
                "patch-1.dll loaded at {loaded:#x}, linked for {:#x}",
                linked.table.new_base_address
            );
        }
        assert_eq!(table.get(&app("HomeState")).map(|e| e.size), Some(4));
        let present = fat
            .base
            .accepted
            .check(&LayoutTable::default(), seams)
            .unwrap();
        assert_eq!(
            fat.base.accepted.check(&table, present.clone()),
            Ok(present)
        );

        // Without its PDB the patch has no table: refused, never passed.
        std::fs::remove_file(session_dir.join("patch-1.pdb")).unwrap();
        let err = pdb_layout::extract(std::slice::from_ref(&linked.path), &fat.base.builder.crates)
            .unwrap_err();
        assert!(
            matches!(&err, HotpatchError::BuilderUnsupported { detail } if detail.contains("/DEBUG")),
            "{err:?}"
        );
    }

    #[test]
    fn a_d2_edit_is_refused_from_the_patch_pdb_table_after_the_link() {
        let mut fat = fat("d2", "2").unwrap();
        fat.edit("d2-field-add");
        let (linked, seams, table) = patch(&mut fat);
        assert!(linked.path.ends_with("patch-1.dll"));
        let present = fat
            .base
            .accepted
            .check(&LayoutTable::default(), seams)
            .unwrap();
        assert_eq!(
            fat.base.accepted.check(&table, present),
            Err(RestartReason::LayoutChanged {
                records: vec![format!("{} changed layout (4 → 8 bytes)", app("HomeState"))]
            })
        );
    }

    #[test]
    fn a_fat_build_without_type_records_is_refused_naming_the_pdb() {
        let err = match fat("line-tables", "line-tables-only") {
            Ok(_) => panic!("a line-tables-only build passed the base gate"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, HotpatchError::BuilderUnsupported { detail }
                if detail.contains("no type records") && detail.contains(".pdb")),
            "{err:?}"
        );
    }
}
