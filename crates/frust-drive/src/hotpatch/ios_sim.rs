//! The iOS simulator hot-patch start: Xcode links the fat image, the
//! builder reads it, and the session reaches the app on the host's
//! loopback.
//!
//! **Fat build.** The simulator run's own `xcodebuild` invocation
//! ([`xcodebuild::build_hot`]) with command-line build settings, which
//! outrank every level of the project, so the template is untouched
//! ([`hot_settings`]). Xcode exports every build setting, command-line ones
//! included, into the environment of the "Build Rust staticlib" script
//! phase, so its `cargo build --target <triple>` runs every workspace
//! member's compile through the capture wrapper
//! (`RUSTC_WORKSPACE_WRAPPER=<frust>`, the scope in
//! `FRUST_HOTPATCH_CAPTURE`). The records keep the allowlisted build
//! environment (no ambient-name list is passed: the script's environment
//! is several hundred Xcode settings, none of which a replay needs beyond
//! `SDKROOT` and `IPHONEOS_DEPLOYMENT_TARGET`, both allowlisted).
//!
//! **Fat rustc flags.** The script runs `cargo build`, not `cargo rustc`,
//! so there is no `--` that reaches the tip alone, and `cargo build` also
//! builds and links the package's bin. Target-scoped rustflags would reach
//! every crate of the simulator triple and sit in every unit's fingerprint
//! (a switch between hot and cold runs recompiles the dependency graph),
//! and on a stock app `-Csave-temps` on every crate left 11 GB in `deps/`
//! and a 910 MB staticlib (1.9 GB and 485 MB without it). Intercepting the
//! link in the script is no better: the bin links after the lib's `cdylib`
//! and overwrites the captured line. So the Xcode build carries the wrapper only, and the
//! fat flags ([`FAT_RUSTFLAGS`]) reach the tip alone afterwards: they are
//! added to its record ([`with_fat_flags`], with the simulator SDK as an
//! `-isysroot` link argument), whose replay (rustc, the
//! `cdylib` link intercepted as no-link) captures the image unit's link
//! line — the tip's saved objects and the rlibs — exactly as a desktop or
//! Android thin build does. Every later replay of the tip reads the same
//! record.
//!
//! **Fat link.** Xcode's link of `Runner` is the fat link
//! ([`xcodebuild::hot_link_settings`]): the project's `OTHER_LDFLAGS`
//! through `$(inherited)`, every member of the staticlib force-loaded
//! (through a response file, [`FORCE_LOAD_RSP`]), the anchor exported,
//! `DEAD_CODE_STRIPPING=NO`, and `ENABLE_DEBUG_DYLIB=NO` so the app's code
//! stays in `Runner` instead of moving to `Runner.debug.dylib`. The builder
//! links nothing at start ([`session::adopt_base`]): the symbol cache reads
//! `build/ios/Build/Products/<configuration>-iphonesimulator/Runner.app/Runner`
//! once [`base_image`] has checked that Xcode honoured the settings — no
//! debug dylib, the anchor defined and exported, and nothing else exported
//! (the export list rides in the same `OTHER_LDFLAGS` value as the
//! force-load, so it proves that value reached the link).
//!
//! **Thin links.** The desktop builder unchanged, Darwin flavor: the tip's
//! replayed `cdylib` link line carries rustc's `-target
//! <arch>-apple-ios<version>-simulator` and the `-isysroot <simulator SDK>`
//! the record gained ([`with_fat_flags`]), which
//! [`thin_link::forwarded_args`](super::thin_link::forwarded_args) keeps
//! for an iOS target, and the stub declares the simulator platform
//! (`PLATFORM_IOSSIMULATOR`), without which ld refuses to link it. A patch
//! is never stripped or rewritten after the link: ld's own ad-hoc
//! signature is what lets the simulator's dyld load it.
//!
//! **Launch.** `ios_run::spawn_hot_session` runs the simulator pipeline
//! with the fat build and the base adopted between the build and `simctl
//! install`, then streams `simctl launch --console-pty`. The app's
//! devtools discovery line is read from that stream; the simulator shares
//! the host's loopback, so the session connects to `127.0.0.1:<port>`
//! directly, and uploads every patch in chunks: the app writes it into its
//! own data container. A start cancelled after the launch kills the stream
//! and terminates the app.
//!
//! **Keys.** The scope and the fat dir carry the simulator triple; the
//! session dir (`session-<crate>-<triple>-<udid>`, [`session_name`]) also
//! carries the simulator's udid, so two simulators never share one.
//!
//! Every surprise fails closed as [`HotpatchError::BuilderUnsupported`]: a
//! physical device, a crate without `staticlib` and `cdylib`, no capture,
//! no anchor, a setting Xcode did not honour. See
//! `docs/CLI_ARCHITECTURE.md`.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use object::read::{Object, ObjectSymbol};
use object::{SymbolKind, SymbolScope};

use crate::build_info::{BuildInfo, BuildMode};
use crate::devices::{Device, Kind, Platform};
use crate::doctor::EnvLookup;
use crate::ios_build::xcodebuild::host_sim_arch;
use crate::ios_run::{self, HotBuild, IosLaunch, simctl, xcodebuild};
use crate::manifest;
use crate::process::ProcessRunner;

use super::android::serial_component;
use super::capture::{self, CAPTURE_ENV, RecordKey, RustcRecord, ScopeInputs, prepare_scope_dir};
use super::fat_link::LinkerFlavor;
use super::graph::{ReplayUnit, WorkspaceGraph};
use super::link_intercept::{LinkAction, LinkMode, linker_arg, read_link_args};
use super::replay::{STRIPPED_ENV, parse_notifications, replay_args, replay_env};
use super::session::{
    self, Announced, AppLink, BaseRequest, Budget, Cancelled, FAT_DIR, FatBase, HotSession,
    RestartReason, SessionHost, StartError, check_debuginfo,
};
use super::symbols::{SymbolCache, Target, parse_object};
use super::{HotpatchError, hotpatch_root};

/// The simulator triple on Apple silicon.
pub const TRIPLE_ARM64: &str = "aarch64-apple-ios-sim";

/// The simulator triple on an Intel host.
pub const TRIPLE_X86_64: &str = "x86_64-apple-ios";

/// The fat rustc flags the tip's record gains ([`with_fat_flags`]).
pub const FAT_RUSTFLAGS: [&str; 2] = ["-Csave-temps=true", "-Clink-dead-code"];

/// The response file in the fat dir that holds the staticlib's force-load
/// ([`xcodebuild::force_load_response`]).
pub const FORCE_LOAD_RSP: &str = "force-load.rsp";

/// The Mach-O header symbol ld exports from an executable without an
/// export list, by its C name (`__mh_execute_header` in the symbol table).
const MH_EXECUTE_HEADER: &str = "_mh_execute_header";

/// The triple the template's script phase builds for this host's
/// simulator (`ARCHS` is pinned to the host arch): `x86_64-apple-ios` on
/// Intel, `aarch64-apple-ios-sim` otherwise.
pub fn triple() -> &'static str {
    if host_sim_arch() == "x86_64" {
        TRIPLE_X86_64
    } else {
        TRIPLE_ARM64
    }
}

/// The session directory's name: the crate, the triple and the
/// simulator's udid as one safe path component.
pub fn session_name(crate_name: &str, triple: &str, udid: &str) -> String {
    format!("session-{crate_name}-{triple}-{}", serial_component(udid))
}

/// The rustflags a scope is keyed on: cargo's global ones, else the
/// triple-scoped `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` (the fat flags never
/// travel as rustflags here).
fn ios_rustflags(env: &dyn EnvLookup, triple: &str) -> Vec<String> {
    let global = session::rustflags(env);
    if !global.is_empty() {
        return global;
    }
    let key = format!(
        "CARGO_TARGET_{}_RUSTFLAGS",
        triple.to_ascii_uppercase().replace(['-', '.'], "_")
    );
    env.get(&key)
        .map(|flags| flags.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Every setting of a hot build, in `xcodebuild` order: the fat link
/// ([`xcodebuild::hot_link_settings`] over the response file
/// `force_load_rsp`), then the capture wrapper and its scope. A response
/// file path Xcode would split or expand (whitespace, quotes, `$`, `\`) is
/// [`HotpatchError::BuilderUnsupported`].
pub fn hot_settings(
    force_load_rsp: &Path,
    frust_exe: &Path,
    scope_dir: &Path,
) -> Result<Vec<(String, String)>, HotpatchError> {
    let rendered = force_load_rsp.to_string_lossy();
    if rendered.contains(|c: char| c.is_whitespace() || "\"'$\\".contains(c)) {
        return Err(HotpatchError::unsupported(format!(
            "the force-load response file `{rendered}` has a character Xcode would split or \
             expand in OTHER_LDFLAGS"
        )));
    }
    let mut settings = xcodebuild::hot_link_settings(force_load_rsp);
    settings.extend(capture::wrapper_env(frust_exe, scope_dir));
    Ok(settings)
}

/// The first of the two `-C link-arg`s that put `-isysroot <sdk>` on the
/// tip's link line ([`with_fat_flags`]).
const ISYSROOT_LINK_ARG: &str = "-Clink-arg=-isysroot";

/// `record` with [`FAT_RUSTFLAGS`] appended, each only when absent (what
/// `cargo rustc -- <flags>` gives the tip on desktop and Android), then
/// `-Clink-arg=-isysroot -Clink-arg=<sdk>` for the simulator SDK the
/// record's `SDKROOT` names (Xcode exports the absolute path to the script
/// phase). rustc 1.98 links a simulator `cdylib` with `-target
/// <arch>-apple-ios<version>-simulator` and no `-isysroot`, leaving the SDK
/// to `SDKROOT` in the environment; on the line itself, every thin link
/// carries it explicitly. No absolute `SDKROOT` is
/// [`HotpatchError::BuilderUnsupported`].
pub fn with_fat_flags(mut record: RustcRecord) -> Result<RustcRecord, HotpatchError> {
    for flag in FAT_RUSTFLAGS {
        if !record.args.iter().any(|arg| arg == flag) {
            record.args.push(flag.to_string());
        }
    }
    if record.args.iter().any(|arg| arg == ISYSROOT_LINK_ARG) {
        return Ok(record);
    }
    let sdk = record
        .envs
        .iter()
        .find(|(name, _)| name == "SDKROOT")
        .map(|(_, value)| value.clone())
        .filter(|sdk| Path::new(sdk).is_absolute())
        .ok_or_else(|| {
            HotpatchError::unsupported(
                "the tip's record carries no absolute SDKROOT for the simulator SDK",
            )
        })?;
    record.args.push(ISYSROOT_LINK_ARG.to_string());
    record.args.push(format!("-Clink-arg={sdk}"));
    Ok(record)
}

/// The symbol cache of `<app>/Runner`, read once the hot build is shown to
/// have honoured its settings: `<app>/Runner.debug.dylib` absent
/// (`ENABLE_DEBUG_DYLIB=NO`), the anchor a defined, exported text symbol of
/// `Runner`, and no other symbol exported but `__mh_execute_header` (the
/// `OTHER_LDFLAGS` value that exports the anchor also force-loads the
/// staticlib). Anything else is [`HotpatchError::BuilderUnsupported`].
pub fn base_image(app: &Path, target: Target) -> Result<SymbolCache, HotpatchError> {
    let debug_dylib = app.join("Runner.debug.dylib");
    if debug_dylib.exists() {
        return Err(HotpatchError::unsupported(format!(
            "Xcode split the app's code into `{}` (ENABLE_DEBUG_DYLIB=NO was not honoured)",
            debug_dylib.display()
        )));
    }
    let image = app.join("Runner");
    let bytes = std::fs::read(&image)
        .map_err(|err| HotpatchError::io(format!("reading `{}`", image.display()), err))?;
    let cache = SymbolCache::from_bytes(&image, &bytes, target)?;
    let what = format!("`{}`", image.display());
    let file = parse_object(&bytes, &what)?;
    let anchor = target.anchor_symbol();
    let header = target.raw_symbol_name(MH_EXECUTE_HEADER);
    let mut anchor_exported = false;
    let mut others = Vec::new();
    for symbol in file.symbols() {
        if symbol.is_undefined() || symbol.scope() != SymbolScope::Dynamic {
            continue;
        }
        match symbol.name() {
            Ok(name) if name == anchor => {
                anchor_exported = symbol.kind() == SymbolKind::Text;
            }
            Ok(name) if name == header => {}
            Ok(name) => others.push(name.to_string()),
            Err(_) => others.push("<unreadable>".to_string()),
        }
    }
    if !anchor_exported {
        return Err(HotpatchError::unsupported(format!(
            "{what} does not export `{anchor}` (the hot build's OTHER_LDFLAGS was not honoured)"
        )));
    }
    if !others.is_empty() {
        others.sort();
        others.truncate(4);
        return Err(HotpatchError::unsupported(format!(
            "{what} exports more than `{anchor}` ({}): the hot build's OTHER_LDFLAGS export \
             list did not apply",
            others.join(", ")
        )));
    }
    Ok(cache)
}

/// The workspace graph of a simulator tip: `package`'s lib must declare
/// `staticlib` (the archive Xcode links) and `cdylib` (the link the tip's
/// replay intercepts for the patch's link line). The graph is keyed on a
/// bin as well, any one serves (the image unit is the lib); a package
/// without one is refused.
fn ios_graph(metadata: &str, package: &str) -> Result<WorkspaceGraph, HotpatchError> {
    let doc: serde_json::Value = serde_json::from_str(metadata).map_err(|err| {
        HotpatchError::unsupported(format!("unreadable `cargo metadata` output: {err}"))
    })?;
    let targets: Vec<&serde_json::Value> = doc
        .get("packages")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter(|p| p.get("name").and_then(|n| n.as_str()) == Some(package))
        .flat_map(|p| p.get("targets").and_then(|t| t.as_array()))
        .flatten()
        .collect();
    let has = |target: &serde_json::Value, value: &str| {
        target
            .get("crate_types")
            .and_then(|list| list.as_array())
            .is_some_and(|list| list.iter().any(|item| item.as_str() == Some(value)))
    };
    if !targets
        .iter()
        .any(|t| has(t, "staticlib") && has(t, "cdylib"))
    {
        return Err(HotpatchError::unsupported(format!(
            "`{package}` declares no lib with both `staticlib` (Xcode's link) and `cdylib` \
             (the captured link line)"
        )));
    }
    let mut bins: Vec<&str> = targets
        .iter()
        .filter(|t| has(t, "bin"))
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .collect();
    bins.sort_unstable();
    let Some(bin) = bins.first() else {
        return Err(HotpatchError::unsupported(format!(
            "`{package}` has no bin target; the hot-patch workspace graph needs one (the \
             scaffolded `src/main.rs` desktop entry)"
        )));
    };
    WorkspaceGraph::from_metadata(metadata, package, Some(bin))
}

/// The tip's record as the Xcode build captured it. A missing record means
/// the script phase did not run cargo through the wrapper (Xcode did not
/// export the capture settings): [`HotpatchError::BuilderUnsupported`].
pub fn captured_record(scope_dir: &Path, key: &RecordKey) -> Result<RustcRecord, HotpatchError> {
    let path = scope_dir.join(key.file_name());
    if !path.is_file() {
        return Err(HotpatchError::unsupported(format!(
            "the Xcode build captured no `{key}` rustc record (`{}`): its script phase did not \
             see the capture settings",
            path.display()
        )));
    }
    Ok(capture::read_record(&path)?.1)
}

/// Replays the tip's record with the fat flags and the simulator SDK added
/// ([`with_fat_flags`], written back so every later replay carries them) and its `cdylib` link
/// intercepted as no-link at `link`, and answers the captured link line.
/// A compile error is [`StartError::FatBuildFailed`] with rustc's
/// diagnostics.
fn capture_tip_link(
    runner: &dyn ProcessRunner,
    graph: &WorkspaceGraph,
    image_unit: &ReplayUnit,
    scope_dir: &Path,
    frust_exe: &Path,
    link: &LinkAction,
) -> Result<Vec<String>, StartError> {
    let key = image_unit.record_key();
    let record = with_fat_flags(captured_record(scope_dir, &key)?)?;
    capture::write_record(scope_dir, &key, &record)?;
    let rustc = record
        .rustc()
        .ok_or_else(|| HotpatchError::unsupported(format!("the `{key}` capture has no rustc")))?
        .to_string();
    let mut args = replay_args(&record)?;
    args.push(linker_arg(frust_exe));
    let mut env = replay_env(&record);
    env.push((
        CAPTURE_ENV.to_string(),
        scope_dir.to_string_lossy().into_owned(),
    ));
    env.extend(link.env_vars());
    let removed: Vec<&str> = STRIPPED_ENV
        .iter()
        .copied()
        .filter(|name| !env.iter().any(|(set, _)| set == name))
        .collect();
    session::remove_stale(&link.args_file)?;
    let cwd = graph.replay_cwd(image_unit);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let output = runner
        .run_streaming_scrubbed(&rustc, &argv, Some(&cwd), &env_refs, &removed, &mut |_| {})
        .map_err(|err| HotpatchError::Process {
            detail: format!("failed to spawn `{rustc}` to replay {image_unit}: {err:#}"),
        })?;
    if !output.success {
        let (_, mut diagnostics) = parse_notifications(&output.stderr, &cwd)?;
        if let Some(report) = link
            .err_file
            .as_ref()
            .and_then(|file| std::fs::read_to_string(file).ok())
        {
            diagnostics.push(report);
        }
        return Err(StartError::FatBuildFailed { diagnostics });
    }
    Ok(read_link_args(&link.args_file)?)
}

/// What to build, launch and attach to.
#[derive(Debug, Clone, Copy)]
pub struct IosSimStart<'a> {
    /// The project root, holding `ios/` and `frust.toml`.
    pub root: &'a Path,
    pub info: &'a BuildInfo,
    /// The tip package: the app's lib crate.
    pub package: &'a str,
    /// A booted simulator.
    pub device: &'a Device,
}

/// A started simulator hot session.
pub struct IosSimHotStart {
    pub session: HotSession,
    /// The launched app and its `simctl launch --console-pty` stream (after
    /// the discovery line). Killing the stream does not stop the app;
    /// `simctl terminate` ([`simctl::terminate`]) does.
    pub launch: IosLaunch,
}

/// What the staging step needs to adopt the base once Xcode has linked it.
struct Staging<'a> {
    graph: WorkspaceGraph,
    image_unit: ReplayUnit,
    target: Target,
    flavor: LinkerFlavor,
    target_dir: PathBuf,
    fat_dir: &'a Path,
    scope_dir: PathBuf,
    link: LinkAction,
    session: String,
}

/// Builds the app fat through Xcode, installs and launches it on the
/// simulator `start.device` and attaches a session to it. `on_line`
/// receives every pipeline line (the build's `[xcodebuild]` output
/// included) and the app's console up to its discovery line (token
/// redacted).
///
/// `Ok(None)` when `cancel` was observed: at a pipeline phase boundary
/// (nothing launched), during the discovery wait or the attach (the stream
/// is killed and the app terminated first). A failure before the build
/// finished is [`StartError::FatBuildFailed`]; one adopting the base is
/// that step's own error; a later one is [`StartError::Launch`].
pub fn start_ios_sim(
    host: &SessionHost<'_>,
    start: &IosSimStart<'_>,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Option<IosSimHotStart>, StartError> {
    let runner: &dyn ProcessRunner = &*host.runner;
    let unsupported =
        |detail: String| StartError::RestartRequired(RestartReason::BuilderUnsupported { detail });
    if start.info.mode != BuildMode::Debug {
        return Err(unsupported(format!(
            "hot patching needs a debug build, not {:?}",
            start.info.mode
        )));
    }
    if start.device.platform != Platform::Ios || start.device.kind != Kind::Simulator {
        return Err(unsupported(format!(
            "`{}` is not an iOS simulator; a physical iOS device stays restart-only",
            start.device.name
        )));
    }
    let manifest =
        manifest::load_optional(start.root).map_err(|err| unsupported(format!("{err:#}")))?;
    let budget = Budget::from_section(manifest.as_ref().and_then(|m| m.hotpatch.as_ref()));

    let triple = triple();
    let metadata =
        super::graph::cargo_metadata(runner, &start.root.join("Cargo.toml"), Some(triple))?;
    let graph = ios_graph(&metadata, start.package)?;
    let image_unit = graph.tip_lib().ok_or_else(|| {
        HotpatchError::unsupported(format!("`{}` has no lib target", start.package))
    })?;
    check_debuginfo(host.env, start.root, graph.workspace_root())?;
    let target_dir = session::target_directory(&metadata)?;
    let rustc_version = capture::rustc_version(runner)?;
    let target = Target::from_triple(triple)?;
    let flavor = LinkerFlavor::for_triple(triple)?;
    let features = start.info.mode.session_cargo_features(true);
    let scope = ScopeInputs {
        tip: start.package.to_string(),
        triple: triple.to_string(),
        profile: "dev".to_string(),
        features: features.iter().map(|f| (*f).to_string()).collect(),
        rustflags: ios_rustflags(host.env, triple),
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
        &capture::fingerprint_dir(&target_dir, Some(triple), "dev"),
        &scope_dir,
        start.package,
        &members,
    )?;

    let fat_dir = hotpatch_root(&target_dir)
        .join(FAT_DIR)
        .join(scope.dir_name()?);
    std::fs::create_dir_all(&fat_dir)
        .map_err(|err| HotpatchError::io(format!("creating `{}`", fat_dir.display()), err))?;
    let crate_name = image_unit.record_key().crate_name;
    let configuration = start.info.mode.xcode_configuration();
    let rsp = fat_dir.join(FORCE_LOAD_RSP);
    let archive = xcodebuild::staticlib_path(start.root, configuration, &crate_name);
    std::fs::write(&rsp, xcodebuild::force_load_response(&archive))
        .map_err(|err| HotpatchError::io(format!("writing `{}`", rsp.display()), err))?;
    let settings = hot_settings(&rsp, &host.frust_exe, &scope_dir)?;
    let link = LinkAction {
        mode: LinkMode::NoLink,
        args_file: fat_dir.join("link-args.json"),
        err_file: Some(fat_dir.join("link-err.txt")),
    };

    let mut staging = Some(Staging {
        graph,
        image_unit,
        target,
        flavor,
        target_dir,
        fat_dir: &fat_dir,
        scope_dir,
        link,
        session: session_name(&crate_name, triple, &start.device.id),
    });
    let mut staged: Option<FatBase> = None;
    let mut failure: Option<StartError> = None;
    let mut built = false;
    let launched = {
        let mut stage = |app: &Path, on_line: &mut dyn FnMut(&str)| -> anyhow::Result<()> {
            built = true;
            let Some(staging) = staging.take() else {
                anyhow::bail!("the hot build staged its base twice");
            };
            match adopt(host, staging, app, on_line) {
                Ok(base) => {
                    staged = Some(base);
                    Ok(())
                }
                Err(err) => {
                    let message = err.to_string();
                    failure = Some(err);
                    Err(anyhow::anyhow!(message))
                }
            }
        };
        let mut hot = HotBuild {
            settings: &settings,
            stage: &mut stage,
        };
        ios_run::spawn_hot_session(
            runner,
            start.root,
            start.device,
            start.info,
            &mut hot,
            on_line,
            cancel,
        )
    };
    let mut launch = match launched {
        Ok(Some(launch)) => launch,
        Ok(None) => return Ok(None),
        Err(err) => {
            return Err(failure.unwrap_or_else(|| {
                let detail = format!("{err:#}");
                if built {
                    StartError::Launch { detail }
                } else {
                    StartError::FatBuildFailed {
                        diagnostics: vec![detail],
                    }
                }
            }));
        }
    };
    let udid = &start.device.id;
    let Some(base) = staged else {
        unwind(runner, udid, &mut launch);
        return Err(StartError::Launch {
            detail: "the app was launched without a staged hot-patch base".to_string(),
        });
    };
    let announced = match session::read_discovery_cancellable(&mut launch.stream, on_line, cancel) {
        Ok(announced) => announced,
        Err(Cancelled) => {
            unwind(runner, udid, &mut launch);
            return Ok(None);
        }
    };
    let app = match announced {
        Announced::Endpoint(discovery) => session::attach_device_app(
            SocketAddr::from((Ipv4Addr::LOCALHOST, discovery.port)),
            discovery.token.as_deref(),
            triple,
        ),
        Announced::Failure(reason) => AppLink::RestartOnly { reason },
        Announced::Exited => {
            unwind(runner, udid, &mut launch);
            return Err(StartError::Launch {
                detail: "the app's console ended before it announced its devtools endpoint"
                    .to_string(),
            });
        }
    };
    if cancel.load(Ordering::SeqCst) {
        drop(app);
        unwind(runner, udid, &mut launch);
        return Ok(None);
    }
    Ok(Some(IosSimHotStart {
        session: session::open_session(base, app, budget),
        launch,
    }))
}

/// The staging step: checks the built image, captures the tip's link line
/// by replaying it with the fat flags, then adopts Xcode's `Runner` as the
/// session's base.
fn adopt(
    host: &SessionHost<'_>,
    staging: Staging<'_>,
    app: &Path,
    on_line: &mut dyn FnMut(&str),
) -> Result<FatBase, StartError> {
    let Staging {
        graph,
        image_unit,
        target,
        flavor,
        target_dir,
        fat_dir,
        scope_dir,
        link,
        session,
    } = staging;
    let cache = base_image(app, target)?;
    let image = cache.path().to_path_buf();
    on_line(&format!(
        "Hot-patch base: `{}`, anchor at {:#x}.",
        image.display(),
        cache.anchor_address()
    ));
    drop(cache);
    on_line(&format!("Capturing the {image_unit} link line…"));
    let link_args = capture_tip_link(
        &*host.runner,
        &graph,
        &image_unit,
        &scope_dir,
        &host.frust_exe,
        &link,
    )?;
    Ok(session::adopt_base(
        host,
        BaseRequest {
            graph,
            link_args,
            image_unit,
            image,
            custom_linker: None,
            upload_strip: None,
            target,
            flavor,
            target_dir,
            archive_dir: fat_dir,
            scope_dir,
            session,
        },
    )?)
}

/// Tears down what a start launched, best-effort: kill the console stream,
/// terminate the app.
fn unwind(runner: &dyn ProcessRunner, udid: &str, launch: &mut IosLaunch) {
    launch.stream.kill();
    simctl::terminate(runner, udid, &launch.bundle_id);
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;

    use object::read::File;
    use object::write::{Object as WriteObject, Symbol, SymbolSection};
    use object::{Endianness, SymbolFlags};

    use super::super::stub::{IOS_SIMULATOR_MIN_VERSION, UndefinedSymbols, build_stub};
    use super::super::symbols::{Format, Os};
    use super::super::thin_link::{ThinLinkRequest, thin_link_args};
    use super::*;
    use crate::build_info::BuildArgs;
    use crate::doctor::FakeEnv;
    use crate::process::FakeProcessRunner;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-hotpatch-ios-sim-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn target() -> Target {
        Target::from_triple(TRIPLE_ARM64).unwrap()
    }

    /// A Mach-O image with `text` symbols, each exported (`Dynamic`) or
    /// hidden (`Linkage`), the way ld leaves an executable linked with an
    /// export list.
    fn image(symbols: &[(&str, SymbolScope)]) -> Vec<u8> {
        let target = target();
        let mut obj = WriteObject::new(
            target.object_format(),
            target.object_architecture(),
            Endianness::Little,
        );
        // Raw names, spelled as the symbol table spells them.
        obj.set_mangling(object::write::Mangling::None);
        let text = obj.section_id(object::write::StandardSection::Text);
        for (name, scope) in symbols {
            let offset = obj.append_section_data(text, &[0x1f, 0x20, 0x03, 0xd5], 4);
            obj.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value: offset,
                size: 4,
                kind: SymbolKind::Text,
                scope: *scope,
                weak: false,
                section: SymbolSection::Section(text),
                flags: SymbolFlags::None,
            });
        }
        obj.write().unwrap()
    }

    fn app_with(dir: &Path, bytes: &[u8]) -> PathBuf {
        let app = dir.join("build/ios/Build/Products/Debug-iphonesimulator/Runner.app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("Runner"), bytes).unwrap();
        app
    }

    #[test]
    fn simulator_triples_are_mach_o_targets_and_a_device_is_not() {
        for (triple, flavor) in [(TRIPLE_ARM64, true), (TRIPLE_X86_64, true)] {
            let target = Target::from_triple(triple).unwrap();
            assert_eq!((target.os, target.format()), (Os::IosSim, Format::MachO));
            assert_eq!(target.anchor_symbol(), "___frust_hotpatch_anchor");
            assert_eq!(
                LinkerFlavor::for_triple(triple).unwrap() == LinkerFlavor::Darwin,
                flavor
            );
        }
        for device in [
            "aarch64-apple-ios",
            "aarch64-apple-ios-macabi",
            "x86_64-apple-ios-sim",
        ] {
            assert!(
                matches!(
                    Target::from_triple(device),
                    Err(HotpatchError::BuilderUnsupported { .. })
                ),
                "{device}"
            );
        }
        assert_eq!(
            triple(),
            if cfg!(target_arch = "x86_64") {
                TRIPLE_X86_64
            } else {
                TRIPLE_ARM64
            }
        );
        assert_eq!(
            session_name("hotapp", TRIPLE_ARM64, "AAAA-BBBB"),
            "session-hotapp-aarch64-apple-ios-sim-AAAA-BBBB"
        );
    }

    /// The fat link then the capture wrapper reach the script phase as
    /// build settings; no rustflags and no linker travel through Xcode.
    #[test]
    fn hot_settings_carry_the_fat_link_then_the_capture_wrapper() {
        let settings = hot_settings(
            Path::new("/t/fat/force-load.rsp"),
            Path::new("/opt/frust/bin/frust"),
            Path::new("/t/scope"),
        )
        .unwrap();
        let expected: Vec<(String, String)> = [
            (
                "OTHER_LDFLAGS",
                "$(inherited) @/t/fat/force-load.rsp \
                 -Wl,-exported_symbol,___frust_hotpatch_anchor",
            ),
            ("DEAD_CODE_STRIPPING", "NO"),
            ("ENABLE_DEBUG_DYLIB", "NO"),
            ("RUSTC_WORKSPACE_WRAPPER", "/opt/frust/bin/frust"),
            ("FRUST_HOTPATCH_CAPTURE", "/t/scope"),
        ]
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .to_vec();
        assert_eq!(settings, expected);
        for bad in ["/t/my fat/f.rsp", "/t/$(X)/f.rsp", "/t/a\"b/f.rsp"] {
            assert!(
                matches!(
                    hot_settings(Path::new(bad), Path::new("/f"), Path::new("/s")),
                    Err(HotpatchError::BuilderUnsupported { .. })
                ),
                "{bad}"
            );
        }
        let env = FakeEnv::new().set(
            "CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUSTFLAGS",
            "-Cfoo -Cbar",
        );
        assert_eq!(ios_rustflags(&env, TRIPLE_ARM64), vec!["-Cfoo", "-Cbar"]);
        let env = env.set("RUSTFLAGS", "-Cglobal");
        assert_eq!(ios_rustflags(&env, TRIPLE_ARM64), vec!["-Cglobal"]);
    }

    /// The cache reads `Runner.app/Runner` and finds the anchor's link-time
    /// address there.
    #[test]
    fn the_symbol_cache_reads_runner_and_finds_the_anchor() {
        let dir = temp_dir("runner");
        let bytes = image(&[
            ("_app_fn", SymbolScope::Linkage),
            ("___frust_hotpatch_anchor", SymbolScope::Dynamic),
            ("__mh_execute_header", SymbolScope::Dynamic),
        ]);
        let app = app_with(&dir, &bytes);
        let cache = base_image(&app, target()).unwrap();
        assert_eq!(cache.path(), app.join("Runner"));
        assert_eq!(
            cache.path(),
            xcodebuild::executable_path(&dir, "Debug"),
            "the hot build's executable"
        );
        let file = File::parse(bytes.as_slice()).unwrap();
        let expected = file
            .symbols()
            .find(|s| s.name() == Ok("___frust_hotpatch_anchor"))
            .unwrap()
            .address();
        assert_eq!(cache.anchor_address(), expected);
        assert_ne!(
            cache.anchor_address(),
            0,
            "the anchor is not the first symbol"
        );
    }

    /// No anchor, an unexported anchor, a wider export list or a debug
    /// dylib beside `Runner` all fail closed.
    #[test]
    fn an_image_that_did_not_honour_the_settings_is_builder_unsupported() {
        let cases: [(&str, Vec<u8>, bool, &str); 4] = [
            (
                "no-anchor",
                image(&[("_app_fn", SymbolScope::Dynamic)]),
                false,
                "defines no",
            ),
            (
                "hidden",
                image(&[("___frust_hotpatch_anchor", SymbolScope::Linkage)]),
                false,
                "does not export",
            ),
            (
                "wide",
                image(&[
                    ("___frust_hotpatch_anchor", SymbolScope::Dynamic),
                    ("_main", SymbolScope::Dynamic),
                ]),
                false,
                "exports more than",
            ),
            (
                "split",
                image(&[("___frust_hotpatch_anchor", SymbolScope::Dynamic)]),
                true,
                "ENABLE_DEBUG_DYLIB",
            ),
        ];
        for (tag, bytes, split, needle) in cases {
            let dir = temp_dir(tag);
            let app = app_with(&dir, &bytes);
            if split {
                std::fs::write(app.join("Runner.debug.dylib"), b"code").unwrap();
            }
            match base_image(&app, target()) {
                Err(HotpatchError::BuilderUnsupported { detail }) => {
                    assert!(detail.contains(needle), "{tag}: {detail}")
                }
                other => panic!("{tag}: expected BuilderUnsupported, got {other:?}"),
            }
        }
    }

    /// No record means the script phase never saw the wrapper; a record
    /// gains the fat flags and the simulator SDK once, at the end.
    #[test]
    fn the_tip_record_must_be_captured_and_gains_the_fat_flags_once() {
        let dir = temp_dir("captured");
        let key = RecordKey {
            crate_name: "hotapp".into(),
            kind: capture::TargetKind::Lib,
        };
        let err = captured_record(&dir, &key).unwrap_err();
        assert!(err.to_string().contains("did not see the capture"), "{err}");
        let sdk = "/X.app/Contents/Developer/Platforms/iPhoneSimulator.platform/Developer/SDKs/\
                   iPhoneSimulator27.0.sdk";
        let record = RustcRecord {
            args: ["rustc", "--crate-name", "hotapp", "-Clink-dead-code"]
                .map(str::to_string)
                .to_vec(),
            envs: vec![("SDKROOT".into(), sdk.into())],
            crate_types: ["cdylib", "staticlib", "rlib"].map(str::to_string).to_vec(),
        };
        capture::write_record(&dir, &key, &record).unwrap();
        let read = captured_record(&dir, &key).unwrap();
        assert_eq!(read, record);
        let fat = with_fat_flags(read).unwrap();
        let sdk_arg = format!("-Clink-arg={sdk}");
        assert_eq!(
            fat.args,
            [
                "rustc",
                "--crate-name",
                "hotapp",
                "-Clink-dead-code",
                "-Csave-temps=true",
                "-Clink-arg=-isysroot",
                &sdk_arg,
            ]
            .map(str::to_string)
            .to_vec()
        );
        assert_eq!(with_fat_flags(fat.clone()).unwrap(), fat);

        for envs in [
            vec![],
            vec![("SDKROOT".into(), "iphonesimulator27.0".into())],
        ] {
            let err = with_fat_flags(RustcRecord {
                envs,
                ..record.clone()
            })
            .unwrap_err();
            assert!(err.to_string().contains("SDKROOT"), "{err}");
        }
    }

    /// The stub a simulator patch links declares the simulator platform.
    #[test]
    fn the_stub_declares_the_simulator_platform() {
        let base = image(&[
            ("_app_fn", SymbolScope::Linkage),
            ("___frust_hotpatch_anchor", SymbolScope::Dynamic),
        ]);
        let cache = SymbolCache::from_bytes("Runner", &base, target()).unwrap();
        let undefined = UndefinedSymbols {
            strong: ["_app_fn".to_string()].into_iter().collect(),
            weak: Default::default(),
        };
        let stub = build_stub(&cache, &undefined, cache.anchor_address() + 0x4000).unwrap();
        let file = object::read::macho::MachOFile64::<Endianness>::parse(stub.as_slice()).unwrap();
        let endian = file.endian();
        let mut commands = file.macho_load_commands().unwrap();
        let mut found = None;
        while let Some(command) = commands.next().unwrap() {
            if let Some(version) = command.build_version().unwrap() {
                found = Some((
                    version.platform.get(endian),
                    version.minos.get(endian),
                    version.sdk.get(endian),
                ));
            }
        }
        assert_eq!(
            found,
            Some((
                object::macho::PLATFORM_IOSSIMULATOR,
                IOS_SIMULATOR_MIN_VERSION,
                IOS_SIMULATOR_MIN_VERSION
            ))
        );
    }

    /// A simulator thin link keeps the `-isysroot <simulator SDK>` the tip's
    /// record adds and rustc's simulator `-target`, links as a dylib and
    /// exports the anchor. The captured line is the shape rustc 1.98 wrote
    /// on the rig (objects, rlibs, frameworks, `-target`, `-o`), plus the
    /// record's two link args.
    #[test]
    fn thin_links_carry_the_simulator_sysroot() {
        let deps = "/t/aarch64-apple-ios-sim/debug/deps";
        let sdk = "/X.app/Contents/Developer/Platforms/iPhoneSimulator.platform/Developer/SDKs/\
                   iPhoneSimulator27.0.sdk";
        let object = format!("{deps}/hotapp-1.hotapp.0.rcgu.o");
        let output = format!("{deps}/libhotapp.dylib");
        let captured: Vec<String> = [
            object.as_str(),
            "-framework",
            "Metal",
            "-liconv",
            "-lSystem",
            "-isysroot",
            sdk,
            "-target",
            "arm64-apple-ios15.0.0-simulator",
            "-o",
            output.as_str(),
            "-dynamiclib",
            "-nodefaultlibs",
        ]
        .map(str::to_string)
        .to_vec();
        let args = thin_link_args(&ThinLinkRequest {
            flavor: LinkerFlavor::Darwin,
            linker: "cc",
            tip_link_args: &captured,
            replayed_rlibs: &[],
            stub_object: Path::new("/t/session/stub-1.o"),
            output: Path::new("/t/session/patch-1.dylib"),
            envs: &[],
        })
        .unwrap();
        let expected: Vec<String> = [
            object.as_str(),
            "/t/session/stub-1.o",
            "-Wl,-dylib",
            "-framework",
            "Metal",
            "-liconv",
            "-lSystem",
            "-isysroot",
            sdk,
            "-target",
            "arm64-apple-ios15.0.0-simulator",
            "-nodefaultlibs",
            "-Wl,-exported_symbol,___frust_hotpatch_anchor",
            "-o",
            "/t/session/patch-1.dylib",
        ]
        .map(str::to_string)
        .to_vec();
        assert_eq!(args, expected);
    }

    #[test]
    fn the_graph_needs_a_staticlib_and_cdylib_lib_and_a_bin() {
        const APP: &str = "path+file:///w/app#0.1.0";
        let metadata = |targets: serde_json::Value| {
            serde_json::json!({
                "packages": [{"id": APP, "name": "app", "source": null,
                              "manifest_path": "/w/app/Cargo.toml", "targets": targets}],
                "workspace_members": [APP],
                "resolve": {"nodes": [{"id": APP, "deps": []}], "root": APP},
                "workspace_root": "/w/app",
            })
            .to_string()
        };
        let lib = |types: &[&str]| {
            serde_json::json!({"name": "app", "kind": types, "crate_types": types,
                "src_path": "/w/app/src/lib.rs"})
        };
        let bin = serde_json::json!({"name": "app", "kind": ["bin"], "crate_types": ["bin"],
            "src_path": "/w/app/src/main.rs"});
        let template = lib(&["cdylib", "staticlib", "rlib"]);
        let graph = ios_graph(&metadata(serde_json::json!([template, bin])), "app").unwrap();
        assert_eq!(
            graph.tip_lib().map(|u| u.record_key().crate_name),
            Some("app".into())
        );
        for types in [&["staticlib", "rlib"][..], &["cdylib", "rlib"][..]] {
            let err =
                ios_graph(&metadata(serde_json::json!([lib(types), bin])), "app").unwrap_err();
            assert!(err.to_string().contains("staticlib"), "{types:?}: {err}");
        }
        let err = ios_graph(&metadata(serde_json::json!([template])), "app").unwrap_err();
        assert!(err.to_string().contains("no bin target"), "{err}");
    }

    fn host(
        runner: &std::sync::Arc<FakeProcessRunner>,
        env: &'static FakeEnv,
    ) -> SessionHost<'static> {
        SessionHost {
            runner: runner.clone(),
            env,
            frust_exe: PathBuf::from("/opt/frust/bin/frust"),
        }
    }

    fn simulator(kind: Kind) -> Device {
        Device {
            id: "AAAA".into(),
            name: "iPhone".into(),
            platform: Platform::Ios,
            kind,
            os_version: None,
            connection_state: None,
        }
    }

    /// A Profile build and a physical device are refused before anything
    /// runs.
    #[test]
    fn unsupported_starts_are_refused_before_anything_runs() {
        let dir = temp_dir("refused");
        let debug = BuildInfo::from_args(BuildArgs::default(), BuildMode::Debug).unwrap();
        let profile = BuildInfo::from_args(BuildArgs::default(), BuildMode::Profile).unwrap();
        let env: &'static FakeEnv = Box::leak(Box::new(FakeEnv::new()));
        let simulator_device = simulator(Kind::Simulator);
        let phone = simulator(Kind::PhysicalDevice);
        let cases = [
            (&profile, &simulator_device, "debug build"),
            (&debug, &phone, "restart-only"),
        ];
        for (info, device, needle) in cases {
            let runner = std::sync::Arc::new(FakeProcessRunner::new());
            let start = IosSimStart {
                root: &dir,
                info,
                package: "app",
                device,
            };
            let err = start_ios_sim(
                &host(&runner, env),
                &start,
                &mut |_| {},
                &AtomicBool::new(false),
            )
            .err()
            .expect("refused");
            match err {
                StartError::RestartRequired(RestartReason::BuilderUnsupported { detail }) => {
                    assert!(detail.contains(needle), "{detail}")
                }
                other => panic!("expected BuilderUnsupported, got {other:?}"),
            }
            assert!(runner.recorded_cwd().is_none(), "nothing ran");
        }
    }
}
