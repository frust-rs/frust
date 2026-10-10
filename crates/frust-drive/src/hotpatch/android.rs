//! The Android hot-patch start (arm64-v8a only): the fat build runs outside
//! Gradle, the fat library is staged where Gradle packages it from, and the
//! session reaches the app through `adb forward`.
//!
//! **Fat build.** `cargo ndk -t arm64-v8a --platform 21 -o
//! <app>/build/android/jniLibs rustc -p <package> --lib --features ... --
//! -Csave-temps=true -Clink-dead-code -Clinker=<frust>` ([`fat_build_command`]),
//! with the capture wrapper (`RUSTC_WORKSPACE_WRAPPER=<frust>`, the scope in
//! `FRUST_HOTPATCH_CAPTURE`) and the `cdylib` link intercepted in **proxy**
//! mode: `frust` records the link line, then forwards the untouched
//! invocation to NDK clang `<triple><api>-clang` ([`NdkToolchain`]), so
//! cargo and cargo-ndk see a real library. The `--` flags reach the tip
//! lib's rustc: verified on NDK r28c (28.2.13676358) with cargo-ndk 4.1.2,
//! where `cargo ndk -t arm64-v8a --platform 30 rustc -v -p frust-hotpatch
//! --lib -- -Csave-temps=true -Clink-dead-code` ran `rustc --crate-name
//! frust_hotpatch ... -C linker=<cargo-ndk> ... -Csave-temps=true
//! -Clink-dead-code -C link-arg=...`; a later `-Clinker` overrides
//! cargo-ndk's own linker (cargo-ndk names itself, then calls
//! `<ndk>/.../clang --target=aarch64-linux-android<api>`). No
//! `CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS` fallback is needed. The
//! API level is cargo-ndk's default, 21, passed explicitly so the proxied
//! linker and the cold `cargoNdkBuild` link at the same level.
//!
//! **Fat link.** The builder re-links the captured line itself through the
//! shared base-image half of a session start (`session::link_base`): Gnu
//! flavor, every workspace rlib's objects force-loaded (`--whole-archive`),
//! [`ANCHOR_SYMBOL`](super::fat_link::ANCHOR_SYMBOL) exported in place of
//! dioxus-cli's `main` (the app is a JNI-entered `cdylib` with no `main`),
//! and identical-code folding turned off ([`fat_image_link_args`]): the
//! Android rustflags fold identical functions (`--icf=all`), and two folded
//! functions share the one base address a jump-table entry redirects. The
//! image is written under the cargo target dir
//! (`<target>/frust-hotpatch/fat/<scope>/lib<crate>.so`), the symbol cache
//! is read from that unstripped file, and only then is it copied over the
//! cargo-ndk copy in `jniLibs/arm64-v8a/` ([`stage_fat_image`]). Gradle
//! strips its packaged copy, which is never read ([`ensure_symbol_source`]).
//!
//! **Launch.** `android_run::spawn_hot_session` runs the usual pipeline with
//! the fat build as its staging step and `./gradlew assembleDebug -x
//! cargoNdkBuild`, installs and launches the app, and streams its logcat.
//! The devtools discovery line is read from that stream; the device port is
//! forwarded to an ephemeral host loopback port (`adb forward`), which is
//! the only endpoint a patch is ever sent to, and always in chunks — never
//! as a host path ([`session::attach_device_app`](super::session)). A
//! start cancelled after `am start` leaves nothing behind: the logcat
//! stream is killed, a forward it allocated removed and the app
//! force-stopped ([`start_android`]).
//!
//! **Keys.** The capture scope (`<tip>-<triple>-<profile>-<hash16>`), the
//! fat dir and the session dir all carry the target triple, so a desktop and
//! an Android session of one project never share one. The session dir
//! (`session-<crate>-<triple>-<serial>`, [`session_name`]) also carries the
//! device's adb serial as one safe path component ([`serial_component`]),
//! so it belongs to one running app: every `stub-<n>.o`, `patch-<n>.so` and
//! `patch-<n>.upload.so` in it is linked against that app's
//! `anchor_runtime` (its ASLR slide), and a session start empties it. Two
//! sessions of one project on two devices sharing it would wipe each
//! other's patches and could send one app a patch whose thunks point into
//! the other's address space, which `apply_patch` cannot detect. The fat
//! dir, the capture scope and the staged `jniLibs/arm64-v8a/lib<crate>.so`
//! stay shared between such sessions: they are keyed on the build inputs
//! alone, identical inputs give identical bytes, and cargo's build lock
//! serializes the fat builds. Thin links run the desktop builder unchanged
//! with the tip lib as the image unit: NDK clang, Gnu thin arguments,
//! anchor exported.
//!
//! **Upload.** The host reads each patch's symbols and builds its jump table
//! from the unstripped `patch-<n>.so`, which stays in the session dir; the
//! app is sent `patch-<n>.upload.so`, a `0600` copy the NDK's `llvm-strip
//! --strip-unneeded` writes beside it ([`strip_for_upload`]). The device's
//! loader needs `.dynsym` only, and DWARF and `.symtab` are most of a debug
//! patch, so this shrinks every `adb forward` upload several-fold.
//!
//! Every surprise fails closed as
//! [`HotpatchError::BuilderUnsupported`]: another ABI, no NDK (or one
//! without `llvm-strip`), no `cdylib` lib, no anchor. See
//! `docs/CLI_ARCHITECTURE.md`.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::android_run::{self, AndroidLaunch, StoppedShort, force_stop};
use crate::build_dirs::BuildLayout;
use crate::build_info::{BuildInfo, BuildMode};
use crate::devices::Device;
use crate::devtools_client::{adb_forward_ephemeral, adb_forward_remove, sha256_hex};
use crate::doctor::EnvLookup;
use crate::host_path;
use crate::manifest;
use crate::process::ProcessRunner;

use super::capture::{self, ScopeInputs, WrapperSetup, prepare_scope_dir};
use super::fat_link::{LinkerFlavor, render, run_linker};
use super::graph::WorkspaceGraph;
use super::link_intercept::{LinkAction, LinkMode, linker_arg, output_path, read_link_args};
use super::session::{
    self, Announced, AppLink, BaseRequest, Budget, Cancelled, FAT_DIR, FatBase, FatBuild,
    HotSession, RestartReason, SessionHost, StartError, check_debuginfo,
};
use super::symbols::Target;
use super::thin_link::restrict_to_owner;
use super::{HotpatchError, hotpatch_root};

/// The one Android target a hot session builds.
pub const TRIPLE: &str = "aarch64-linux-android";

/// [`TRIPLE`]'s ABI name, as cargo-ndk, Gradle and `getprop` spell it.
pub const ABI: &str = "arm64-v8a";

/// The NDK API level the fat build links at: cargo-ndk's own default, which
/// the template's `cargoNdkBuild` task relies on.
pub const API_LEVEL: u32 = 21;

/// The identical-code-folding setting the fat image links with.
const ICF_NONE: &str = "-Wl,--icf=none";

/// The NDK pieces a hot session needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NdkToolchain {
    /// The NDK root, handed to cargo-ndk as `ANDROID_NDK_HOME` so it uses
    /// this very NDK.
    pub home: PathBuf,
    /// `<home>/toolchains/llvm/prebuilt/<host>/bin/aarch64-linux-android21-clang`:
    /// the proxied linker of the fat build, and the fat and thin links'
    /// driver.
    pub clang: PathBuf,
    /// `llvm-strip` beside [`clang`](Self::clang): writes the copy of each
    /// patch the app is sent ([`strip_for_upload`]).
    pub strip: PathBuf,
}

impl NdkToolchain {
    /// The NDK cargo-ndk would pick: `ANDROID_NDK_HOME`, else
    /// `ANDROID_NDK_ROOT`, else the highest version under
    /// `<sdk>/ndk/` of `ANDROID_HOME` or `ANDROID_SDK_ROOT`.
    pub fn resolve(env: &dyn EnvLookup) -> Result<Self, HotpatchError> {
        Self::at(&ndk_home(env)?, host_tag()?)
    }

    /// The toolchain of the NDK at `home` for prebuilt host `host_tag`
    /// (`darwin-x86_64`, `linux-x86_64`). A missing clang wrapper or
    /// `llvm-strip` is [`HotpatchError::BuilderUnsupported`] naming it.
    pub fn at(home: &Path, host_tag: &str) -> Result<Self, HotpatchError> {
        let bin = home
            .join("toolchains")
            .join("llvm")
            .join("prebuilt")
            .join(host_tag)
            .join("bin");
        let clang = bin.join(format!("{TRIPLE}{API_LEVEL}-clang"));
        let strip = bin.join(format!("llvm-strip{}", std::env::consts::EXE_SUFFIX));
        for tool in [&clang, &strip] {
            if !tool.is_file() {
                return Err(HotpatchError::unsupported(format!(
                    "the NDK at `{}` has no `{}`",
                    home.display(),
                    tool.display()
                )));
            }
        }
        Ok(Self {
            home: home.to_path_buf(),
            clang,
            strip,
        })
    }
}

/// The NDK's prebuilt host directory for this host. The NDK ships x86_64
/// host tools only (they run under Rosetta on Apple silicon).
fn host_tag() -> Result<&'static str, HotpatchError> {
    if cfg!(target_os = "macos") {
        Ok("darwin-x86_64")
    } else if cfg!(target_os = "linux") {
        Ok("linux-x86_64")
    } else {
        Err(HotpatchError::unsupported(format!(
            "no Android hot-patch toolchain on a `{}` host",
            std::env::consts::OS
        )))
    }
}

fn ndk_home(env: &dyn EnvLookup) -> Result<PathBuf, HotpatchError> {
    let set = |key: &str| env.get(key).filter(|value| !value.is_empty());
    if let Some(home) = set("ANDROID_NDK_HOME").or_else(|| set("ANDROID_NDK_ROOT")) {
        return Ok(PathBuf::from(home));
    }
    for sdk in ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .into_iter()
        .filter_map(set)
    {
        if let Some(ndk) = highest_ndk(&Path::new(&sdk).join("ndk")) {
            return Ok(ndk);
        }
    }
    Err(HotpatchError::unsupported(
        "no Android NDK: set ANDROID_NDK_HOME, or install one under `$ANDROID_HOME/ndk`",
    ))
}

/// The subdirectory of `dir` with the highest dotted version name.
fn highest_ndk(dir: &Path) -> Option<PathBuf> {
    let version = |name: &str| -> Option<Vec<u64>> {
        name.split('.').map(|part| part.parse().ok()).collect()
    };
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            Some((version(&name)?, entry.path()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path)
}

/// `cargo ndk -t arm64-v8a --platform 21 -o <jni_libs> rustc -p <package>
/// --lib --message-format json-diagnostic-rendered-ansi --features ... --
/// -Csave-temps=true -Clink-dead-code -Clinker=<frust>`, with `defines`
/// (the `--define`s, as the Gradle task threads them), `ANDROID_NDK_HOME`,
/// the wrapper variables, `link`'s variables, then
/// [`capture::AMBIENT_ENV`] — the desktop
/// [`fat_build_command`](super::session::fat_build_command)'s contract with
/// cargo-ndk in front and the `cdylib` lib as the tip target.
pub fn fat_build_command(
    package: &str,
    features: &[&str],
    defines: &[(String, String)],
    jni_libs: &Path,
    ndk: &NdkToolchain,
    wrapper: WrapperSetup<'_>,
    link: &LinkAction,
) -> FatBuild {
    let WrapperSetup {
        frust_exe,
        scope_dir,
        ambient_names,
    } = wrapper;
    let render = |path: &Path| host_path::simplify(path).to_string_lossy().into_owned();
    let mut args: Vec<String> = vec![
        "ndk".to_string(),
        "-t".to_string(),
        ABI.to_string(),
        "--platform".to_string(),
        API_LEVEL.to_string(),
        "-o".to_string(),
        render(jni_libs),
    ];
    args.extend(
        [
            "rustc",
            "-p",
            package,
            "--lib",
            "--message-format",
            "json-diagnostic-rendered-ansi",
        ]
        .map(str::to_string),
    );
    for feature in features {
        args.push("--features".to_string());
        args.push((*feature).to_string());
    }
    args.push("--".to_string());
    args.push("-Csave-temps=true".to_string());
    args.push("-Clink-dead-code".to_string());
    args.push(linker_arg(frust_exe));
    let mut env = defines.to_vec();
    env.push(("ANDROID_NDK_HOME".to_string(), render(&ndk.home)));
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

/// Whether one item of a `-Wl,` list sets identical-code folding.
fn is_icf(item: &str) -> bool {
    item.starts_with("--icf=") || item.starts_with("-icf=") || item == "--icf" || item == "-icf"
}

/// The captured `cdylib` link line as the fat image links it: every
/// identical-code-folding setting removed (from a `-Wl,` list, dropping the
/// argument once it is empty) and [`ICF_NONE`] appended. The fat link then
/// adds the force-load and the anchor export ([`super::fat_link::fat_link_args`]).
pub fn fat_image_link_args(captured: Vec<String>) -> Vec<String> {
    let mut args: Vec<String> = captured
        .into_iter()
        .filter_map(|arg| {
            let Some(list) = arg.strip_prefix("-Wl,") else {
                return Some(arg);
            };
            let mut items = list.split(',').peekable();
            let mut kept = Vec::new();
            while let Some(item) = items.next() {
                if item == "--icf" || item == "-icf" {
                    items.next();
                } else if !is_icf(item) {
                    kept.push(item);
                }
            }
            (!kept.is_empty()).then(|| format!("-Wl,{}", kept.join(",")))
        })
        .collect();
    args.push(ICF_NONE.to_string());
    args
}

/// `lib<crate>.so`: the file name cargo, cargo-ndk and `System.loadLibrary`
/// give the app's `cdylib`.
pub fn library_file_name(crate_name: &str) -> String {
    format!("lib{crate_name}.so")
}

/// The session directory's name: the crate and the triple, so a desktop
/// session of the same project (`session-<bin>`) never shares it, and the
/// device's serial ([`serial_component`]), so a session of the same project
/// on another device never shares it either.
pub fn session_name(crate_name: &str, serial: &str) -> String {
    format!("session-{crate_name}-{TRIPLE}-{}", serial_component(serial))
}

/// The longest [`serial_component`].
pub const SERIAL_COMPONENT_MAX: usize = 48;

/// An adb serial as one safe path component: non-empty, at most
/// [`SERIAL_COMPONENT_MAX`] bytes, only `[A-Za-z0-9_-]`. A serial already of
/// that form is kept as is (USB `0A1B2C3D`, `emulator-5554`). Otherwise
/// (`192.168.1.5:5555`, an over-long mDNS name) every other character
/// becomes `-`, the result is cut to fit, and the first 8 hex digits of the
/// serial's SHA-256 are appended, so two serials that map alike still name
/// two directories. An empty serial is `no-serial`.
pub fn serial_component(serial: &str) -> String {
    if serial.is_empty() {
        return "no-serial".to_string();
    }
    let safe = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if serial.len() <= SERIAL_COMPONENT_MAX && serial.chars().all(safe) {
        return serial.to_string();
    }
    let digest = sha256_hex(serial.as_bytes());
    let mapped: String = serial
        .chars()
        .map(|c| if safe(c) { c } else { '-' })
        .take(SERIAL_COMPONENT_MAX - 9)
        .collect();
    format!("{mapped}-{}", &digest[..8])
}

/// Refuses a symbol-cache source that is not under the cargo target dir:
/// the cache must come from the unstripped fat image, never from the
/// `jniLibs` copy or the APK Gradle packages (and strips).
pub fn ensure_symbol_source(source: &Path, target_dir: &Path) -> Result<(), HotpatchError> {
    if host_path::is_under(source, target_dir) {
        Ok(())
    } else {
        Err(HotpatchError::unsupported(format!(
            "the symbol cache must read the unstripped image under `{}`, not `{}`",
            target_dir.display(),
            source.display()
        )))
    }
}

/// Copies the fat image over `<abi_dir>/<file_name>` (the copy cargo-ndk
/// staged from the proxied link), through a partial file and a rename, so
/// Gradle never packages a half-written library. Returns the staged path.
pub fn stage_fat_image(
    image: &Path,
    abi_dir: &Path,
    file_name: &str,
) -> Result<PathBuf, HotpatchError> {
    std::fs::create_dir_all(abi_dir)
        .map_err(|err| HotpatchError::io(format!("creating `{}`", abi_dir.display()), err))?;
    let staged = abi_dir.join(file_name);
    let partial = abi_dir.join(format!(".{file_name}.partial"));
    std::fs::copy(image, &partial)
        .and_then(|_| std::fs::rename(&partial, &staged))
        .map_err(|err| {
            HotpatchError::io(
                format!("staging `{}` as `{}`", image.display(), staged.display()),
                err,
            )
        })?;
    Ok(staged)
}

/// `patch-<n>.upload.so` beside the linked `patch-<n>.so`: the copy the app
/// is sent.
pub fn upload_path(patch: &Path) -> PathBuf {
    patch.with_extension("upload.so")
}

/// Writes [`upload_path`]`(patch)`: `strip --strip-unneeded` drops DWARF and
/// `.symtab` (the host keeps reading both from `patch`) and keeps the
/// `.dynsym` the device's loader needs; the copy is restricted to its owner
/// like the patch. A failed strip is [`HotpatchError::BuilderUnsupported`],
/// one that cannot be spawned [`HotpatchError::Process`]: the session never
/// falls back to sending the unstripped image.
pub fn strip_for_upload(
    runner: &dyn ProcessRunner,
    strip: &Path,
    patch: &Path,
) -> Result<PathBuf, HotpatchError> {
    let upload = upload_path(patch);
    let args = vec![
        "--strip-unneeded".to_string(),
        "-o".to_string(),
        render(&upload),
        render(patch),
    ];
    run_linker(runner, &render(strip), &args, &[], "stripping the patch")?;
    restrict_to_owner(&upload)?;
    Ok(upload)
}

/// The rustflags a scope is keyed on: cargo's global ones, else the
/// triple-scoped `CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS`.
fn android_rustflags(env: &dyn EnvLookup) -> Vec<String> {
    let global = session::rustflags(env);
    if !global.is_empty() {
        return global;
    }
    let key = format!(
        "CARGO_TARGET_{}_RUSTFLAGS",
        TRIPLE.to_ascii_uppercase().replace('-', "_")
    );
    env.get(&key)
        .map(|flags| flags.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The workspace graph of an Android tip: `package` must declare a `cdylib`
/// lib (the app). The graph is keyed on a bin as well; the image is the lib,
/// so any bin serves (the builder drops bins for a lib image), but a
/// package without one is refused.
fn android_graph(metadata: &str, package: &str) -> Result<WorkspaceGraph, HotpatchError> {
    let doc: serde_json::Value = serde_json::from_str(metadata).map_err(|err| {
        HotpatchError::unsupported(format!("unreadable `cargo metadata` output: {err}"))
    })?;
    let members: Vec<&str> = doc
        .get("workspace_members")
        .and_then(|m| m.as_array())
        .map(|ids| ids.iter().filter_map(|id| id.as_str()).collect())
        .unwrap_or_default();
    let tip = doc
        .get("packages")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .find(|p| {
            p.get("name").and_then(|n| n.as_str()) == Some(package)
                && p.get("id")
                    .and_then(|id| id.as_str())
                    .is_some_and(|id| members.contains(&id))
        })
        .ok_or_else(|| {
            HotpatchError::unsupported(format!(
                "the tip package `{package}` is not a workspace member"
            ))
        })?;
    let targets: Vec<&serde_json::Value> = tip
        .get("targets")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
        .collect();
    let has = |target: &serde_json::Value, field: &str, value: &str| {
        target
            .get(field)
            .and_then(|list| list.as_array())
            .is_some_and(|list| list.iter().any(|item| item.as_str() == Some(value)))
    };
    if !targets
        .iter()
        .any(|t| has(t, "kind", "cdylib") || has(t, "crate_types", "cdylib"))
    {
        return Err(HotpatchError::unsupported(format!(
            "`{package}` declares no `cdylib` lib; an Android app is its lib's `cdylib`"
        )));
    }
    let mut bins: Vec<&str> = targets
        .iter()
        .filter(|t| has(t, "kind", "bin"))
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

/// What to build, launch and attach to.
#[derive(Debug, Clone, Copy)]
pub struct AndroidStart<'a> {
    /// The project root: cargo's working directory, holding `android/`.
    pub root: &'a Path,
    pub info: &'a BuildInfo,
    /// The tip package: the app's `cdylib` crate.
    pub package: &'a str,
    pub device: &'a Device,
}

/// A started Android hot session.
pub struct AndroidHotStart {
    pub session: HotSession,
    /// The launched app and its logcat stream (after the discovery line).
    pub launch: AndroidLaunch,
    /// The host port `adb forward` allocated for the devtools endpoint, when
    /// one was; the caller removes it
    /// ([`adb_forward_remove`](crate::devtools_client::adb_forward_remove))
    /// when the session ends.
    pub forward_port: Option<u16>,
}

/// Builds the app fat outside Gradle, packages and launches it on `device`
/// and attaches a session through `adb forward`. `on_line` receives every
/// pipeline line, the build's rendered diagnostics, and the app's logcat
/// up to its discovery line (token redacted).
///
/// `Ok(None)` when `cancel` was observed before the session was attached:
/// at a pipeline phase boundary, during the discovery wait, or once the
/// forward and attach are done. Whatever the start had launched by then is
/// torn down first, best-effort: the logcat stream is killed, the forward
/// removed and the launched package force-stopped. A pipeline that issued
/// `am start` and then ended without a stream — at a later phase boundary,
/// or failing because the Ctrl-C that raised `cancel` also killed the
/// in-flight `adb` — names the package it launched ([`StoppedShort`]),
/// which is force-stopped when `cancel` is raised ([`settle_pipeline`]).
pub fn start_android(
    host: &SessionHost<'_>,
    start: &AndroidStart<'_>,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Option<AndroidHotStart>, StartError> {
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
    let ndk = NdkToolchain::resolve(host.env)?;

    let mut staged: Option<FatBase> = None;
    let mut failure: Option<StartError> = None;
    let launched = {
        let mut stage = |abi: &str, on_line: &mut dyn FnMut(&str)| -> anyhow::Result<()> {
            match build_and_stage(host, start, &ndk, abi, on_line) {
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
        android_run::spawn_hot_session_with_env(
            runner,
            start.root,
            start.device,
            start.info,
            on_line,
            cancel,
            &mut stage,
            host.env,
        )
    };
    let Some(mut launch) = settle_pipeline(runner, &start.device.id, launched, failure, cancel)?
    else {
        return Ok(None);
    };
    let Some(base) = staged else {
        launch.stream.kill();
        return Err(StartError::Launch {
            detail: "the app was launched without a staged hot-patch image".to_string(),
        });
    };

    let Some((app, forward_port)) =
        attach_launched(runner, &start.device.id, &mut launch, on_line, cancel)?
    else {
        return Ok(None);
    };
    Ok(Some(AndroidHotStart {
        session: session::open_session(base, app, budget),
        launch,
        forward_port,
    }))
}

/// The run pipeline's answer to a hot start: the launched app, `Ok(None)`
/// for a cancel, or the failure (`failure`, the staging step's own error,
/// when that is what stopped it). With `cancel` raised, a package the
/// pipeline issued `am start` for but never streamed is force-stopped
/// first, whether it answered a cancel or a failure.
fn settle_pipeline(
    runner: &dyn ProcessRunner,
    serial: &str,
    launched: Result<AndroidLaunch, StoppedShort>,
    failure: Option<StartError>,
    cancel: &AtomicBool,
) -> Result<Option<AndroidLaunch>, StartError> {
    let stopped = match launched {
        Ok(launch) => return Ok(Some(launch)),
        Err(stopped) => stopped,
    };
    if cancel.load(Ordering::SeqCst) {
        stopped.stop_launched(runner, serial);
    }
    match stopped.error {
        None => Ok(None),
        Some(err) => Err(failure.unwrap_or_else(|| StartError::Launch {
            detail: format!("{err:#}"),
        })),
    }
}

/// Waits for the launched app's discovery line and attaches through an
/// `adb forward` on `serial`, answering the link and the forwarded host
/// port. `Ok(None)` when `cancel` was observed during the wait, the forward
/// or the attach: the stream is killed, the forward removed and the app
/// force-stopped first ([`unwind`]).
fn attach_launched(
    runner: &dyn ProcessRunner,
    serial: &str,
    launch: &mut AndroidLaunch,
    on_line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Option<(AppLink, Option<u16>)>, StartError> {
    let mut forward_port = None;
    let announced = match session::read_discovery_cancellable(&mut launch.stream, on_line, cancel) {
        Ok(announced) => announced,
        Err(Cancelled) => {
            unwind(runner, serial, launch, None);
            return Ok(None);
        }
    };
    let app = match announced {
        Announced::Endpoint(discovery) => {
            match adb_forward_ephemeral(runner, serial, discovery.port) {
                Ok(port) => {
                    forward_port = Some(port);
                    session::attach_device_app(
                        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                        discovery.token.as_deref(),
                        TRIPLE,
                    )
                }
                Err(err) => AppLink::RestartOnly {
                    reason: format!("forwarding the devtools port failed: {err:#}"),
                },
            }
        }
        Announced::Failure(reason) => AppLink::RestartOnly { reason },
        Announced::Exited => {
            launch.stream.kill();
            return Err(StartError::Launch {
                detail: "the app's log ended before it announced its devtools endpoint".to_string(),
            });
        }
    };
    if cancel.load(Ordering::SeqCst) {
        // The devtools link goes before its forward does.
        drop(app);
        unwind(runner, serial, launch, forward_port);
        return Ok(None);
    }
    Ok(Some((app, forward_port)))
}

/// Tears down what a cancelled start launched, best-effort (a device that
/// went away or an app already gone is a normal end): kill the logcat
/// stream, remove the forward when one was allocated, force-stop the app.
fn unwind(
    runner: &dyn ProcessRunner,
    serial: &str,
    launch: &mut AndroidLaunch,
    forward_port: Option<u16>,
) {
    launch.stream.kill();
    if let Some(port) = forward_port {
        let _ = adb_forward_remove(runner, serial, port);
    }
    force_stop(runner, serial, &launch.package);
}

/// The staging step [`start_android`] hands the run pipeline: the fat
/// build, the fat link and base image, then the copy into `jniLibs`.
fn build_and_stage(
    host: &SessionHost<'_>,
    start: &AndroidStart<'_>,
    ndk: &NdkToolchain,
    abi: &str,
    on_line: &mut dyn FnMut(&str),
) -> Result<FatBase, StartError> {
    if abi != ABI {
        return Err(HotpatchError::unsupported(format!(
            "Android hot patching builds {ABI} only; the device runs `{abi}`"
        ))
        .into());
    }
    let runner: &dyn ProcessRunner = &*host.runner;
    let metadata =
        super::graph::cargo_metadata(runner, &start.root.join("Cargo.toml"), Some(TRIPLE))?;
    let graph = android_graph(&metadata, start.package)?;
    let image_unit = graph.tip_lib().ok_or_else(|| {
        HotpatchError::unsupported(format!("`{}` has no lib target", start.package))
    })?;
    check_debuginfo(host.env, start.root, graph.workspace_root())?;
    let target_dir = session::target_directory(&metadata)?;

    let rustc_version = capture::rustc_version(runner)?;
    let target = Target::from_triple(TRIPLE)?;
    let flavor = LinkerFlavor::for_triple(TRIPLE)?;
    let features = start.info.mode.session_cargo_features(true);
    let scope = ScopeInputs {
        tip: start.package.to_string(),
        triple: TRIPLE.to_string(),
        profile: "dev".to_string(),
        features: features.iter().map(|f| (*f).to_string()).collect(),
        rustflags: android_rustflags(host.env),
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
        &capture::fingerprint_dir(&target_dir, Some(TRIPLE), "dev"),
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
        mode: LinkMode::Proxy {
            linker: ndk.clang.clone(),
        },
        args_file: fat_dir.join("link-args.json"),
        err_file: Some(fat_dir.join("link-err.txt")),
    };
    session::remove_stale(&link.args_file)?;
    let jni_libs = start.root.join(BuildLayout::android_jni_libs());
    let mut defines: Vec<(String, String)> = start
        .info
        .defines
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    defines.sort();
    let fat = fat_build_command(
        start.package,
        features,
        &defines,
        &jni_libs,
        ndk,
        WrapperSetup {
            frust_exe: &host.frust_exe,
            scope_dir: &scope_dir,
            ambient_names: &capture::ambient_env_names(),
        },
        &link,
    );
    session::run_fat_build(runner, &fat, start.root, on_line)?;

    let link_args = fat_image_link_args(read_link_args(&link.args_file)?);
    let crate_name = image_unit.record_key().crate_name;
    let file_name = library_file_name(&crate_name);
    let linked = output_path(&link_args)?;
    if linked.file_name().and_then(|name| name.to_str()) != Some(file_name.as_str()) {
        return Err(HotpatchError::unsupported(format!(
            "the captured `cdylib` link writes `{}`, not `{file_name}`",
            linked.display()
        ))
        .into());
    }
    let base = session::link_base(
        host,
        BaseRequest {
            graph,
            link_args,
            image_unit,
            image: fat_dir.join(&file_name),
            custom_linker: Some(ndk.clang.clone()),
            upload_strip: Some(ndk.strip.clone()),
            target,
            flavor,
            target_dir: target_dir.clone(),
            archive_dir: &fat_dir,
            scope_dir,
            session: session_name(&crate_name, &start.device.id),
        },
    )?;
    ensure_symbol_source(base.symbol_source(), &target_dir)?;
    let staged = stage_fat_image(base.image(), &jni_libs.join(ABI), &file_name)?;
    on_line(&format!(
        "Staged the hot-patch image `{}` for packaging.",
        staged.display()
    ));
    on_line(&format!(
        "Patches are sent stripped by `{}`; each unstripped `patch-<n>.so` stays on this host.",
        ndk.strip.display()
    ));
    Ok(base)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::super::fat_link::{self, FatLinkRequest, linker_program};
    use super::super::symbols::fixtures::{Def, object};
    use super::super::symbols::{ANCHOR_SYMBOL, SymbolCache};
    use super::super::thin_link::{self, ThinLinkRequest};
    use super::*;
    use crate::doctor::FakeEnv;
    use crate::process::{FakeProcessRunner, Output};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-hotpatch-android-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Canonical, so it compares equal to paths rustc and the archive
        // writer canonicalise (macOS `/var` -> `/private/var`).
        dir.canonicalize().unwrap()
    }

    fn target() -> Target {
        Target::from_triple(TRIPLE).unwrap()
    }

    /// An NDK tree holding only the per-API clang wrapper and `llvm-strip`.
    fn fake_ndk(root: &Path) -> NdkToolchain {
        let bin = root.join("toolchains/llvm/prebuilt/darwin-x86_64/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("aarch64-linux-android21-clang"), "#!/bin/sh\n").unwrap();
        let strip = format!("llvm-strip{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(bin.join(strip), "#!/bin/sh\n").unwrap();
        NdkToolchain::at(root, "darwin-x86_64").unwrap()
    }

    fn ok() -> Output {
        Output {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_ndk_resolves_from_its_home_or_the_sdks_highest_version() {
        let dir = temp_dir("ndk");
        let explicit = dir.join("explicit");
        let ndk = fake_ndk(&explicit);
        assert_eq!(
            ndk.clang,
            explicit
                .join("toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android21-clang")
        );
        let env = FakeEnv::new().set("ANDROID_NDK_HOME", &explicit.to_string_lossy());
        assert_eq!(ndk_home(&env).unwrap(), explicit);

        let sdk = dir.join("sdk");
        for version in ["9.9.1", "27.0.12077973", "28.2.13676358", "not-a-version"] {
            std::fs::create_dir_all(sdk.join("ndk").join(version)).unwrap();
        }
        let env = FakeEnv::new().set("ANDROID_HOME", &sdk.to_string_lossy());
        assert_eq!(ndk_home(&env).unwrap(), sdk.join("ndk/28.2.13676358"));

        let err = ndk_home(&FakeEnv::new()).unwrap_err();
        assert!(err.to_string().contains("ANDROID_NDK_HOME"), "{err}");
        let err = NdkToolchain::at(&sdk.join("ndk/27.0.12077973"), "darwin-x86_64").unwrap_err();
        assert!(
            matches!(err, HotpatchError::BuilderUnsupported { .. }),
            "{err:?}"
        );
    }

    /// An NDK without `llvm-strip` fails closed, naming the missing tool,
    /// rather than leaving the session to send unstripped patches.
    #[test]
    fn an_ndk_without_llvm_strip_is_refused_naming_the_tool() {
        let dir = temp_dir("no-strip");
        let ndk = fake_ndk(&dir.join("ndk"));
        assert_eq!(
            ndk.strip,
            ndk.clang
                .with_file_name(format!("llvm-strip{}", std::env::consts::EXE_SUFFIX))
        );
        std::fs::remove_file(&ndk.strip).unwrap();
        let err = NdkToolchain::at(&dir.join("ndk"), "darwin-x86_64").unwrap_err();
        match &err {
            HotpatchError::BuilderUnsupported { detail } => {
                assert!(
                    detail.contains(&ndk.strip.display().to_string()),
                    "{detail}"
                )
            }
            other => panic!("expected BuilderUnsupported, got {other:?}"),
        }
    }

    /// The upload copy is `llvm-strip --strip-unneeded -o
    /// patch-<n>.upload.so patch-<n>.so`, left owner-only; the linked patch
    /// is untouched. A failed strip is refused, never skipped.
    #[test]
    fn the_upload_copy_is_strip_unneeded_beside_the_patch_and_owner_only() {
        let dir = temp_dir("strip");
        let ndk = fake_ndk(&dir.join("ndk"));
        let patch = dir.join("session").join("patch-3.so");
        std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
        std::fs::write(&patch, b"unstripped").unwrap();
        let upload = upload_path(&patch);
        assert_eq!(upload, dir.join("session").join("patch-3.upload.so"));
        // What llvm-strip leaves under a `022` umask.
        std::fs::write(&upload, b"stripped").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&upload, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let path = |p: &Path| p.to_string_lossy().into_owned();
        let argv = format!(
            "{} --strip-unneeded -o {} {}",
            path(&ndk.strip),
            path(&upload),
            path(&patch)
        );
        let runner = FakeProcessRunner::new().with(argv.clone(), ok());
        assert_eq!(
            strip_for_upload(&runner, &ndk.strip, &patch).unwrap(),
            upload
        );
        assert_eq!(std::fs::read(&patch).unwrap(), b"unstripped");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&upload).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the upload copy is owner-only");
        }

        let failed = Output {
            success: false,
            stdout: String::new(),
            stderr: "llvm-strip: error: truncated file".to_string(),
        };
        let runner = FakeProcessRunner::new().with(argv, failed);
        let err = strip_for_upload(&runner, &ndk.strip, &patch).unwrap_err();
        assert!(
            matches!(err, HotpatchError::BuilderUnsupported { .. }),
            "{err:?}"
        );
    }

    /// The fat build runs cargo-ndk's `rustc` passthrough (the verified
    /// form in the module doc) with the capture wrapper and the link
    /// proxied to the NDK clang.
    #[test]
    fn the_fat_build_is_cargo_ndk_rustc_with_the_capture_and_proxy_link_env() {
        let ndk = NdkToolchain {
            home: PathBuf::from("/sdk/ndk/28.2.13676358"),
            clang: PathBuf::from("/sdk/ndk/28.2.13676358/bin/aarch64-linux-android21-clang"),
            strip: PathBuf::from("/sdk/ndk/28.2.13676358/bin/llvm-strip"),
        };
        let link = LinkAction {
            mode: LinkMode::Proxy {
                linker: ndk.clang.clone(),
            },
            args_file: PathBuf::from("/t/fat/link-args.json"),
            err_file: Some(PathBuf::from("/t/fat/link-err.txt")),
        };
        let fat = fat_build_command(
            "my-app",
            &["frust/perf-trace", "frust/devtools", "frust/hotpatch"],
            &[("APP_FLAVOR".to_string(), "dev".to_string())],
            Path::new("/w/my-app/build/android/jniLibs"),
            &ndk,
            WrapperSetup {
                frust_exe: Path::new("/opt/frust/bin/frust"),
                scope_dir: Path::new("/t/scope"),
                ambient_names: &["PATH".to_string()],
            },
            &link,
        );
        assert_eq!(fat.program, "cargo");
        assert_eq!(
            fat.args.join(" "),
            "ndk -t arm64-v8a --platform 21 -o /w/my-app/build/android/jniLibs rustc -p my-app \
             --lib --message-format json-diagnostic-rendered-ansi --features frust/perf-trace \
             --features frust/devtools --features frust/hotpatch -- -Csave-temps=true \
             -Clink-dead-code -Clinker=/opt/frust/bin/frust"
        );
        let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(
            fat.env,
            vec![
                pair("APP_FLAVOR", "dev"),
                pair("ANDROID_NDK_HOME", "/sdk/ndk/28.2.13676358"),
                pair("RUSTC_WORKSPACE_WRAPPER", "/opt/frust/bin/frust"),
                pair("FRUST_HOTPATCH_CAPTURE", "/t/scope"),
                pair("FRUST_HOTPATCH_LINK", "proxy"),
                pair("FRUST_HOTPATCH_LINK_ARGS_FILE", "/t/fat/link-args.json"),
                pair("FRUST_HOTPATCH_LINK_ERR_FILE", "/t/fat/link-err.txt"),
                pair(
                    "FRUST_HOTPATCH_LINKER",
                    "/sdk/ndk/28.2.13676358/bin/aarch64-linux-android21-clang"
                ),
                pair(
                    "FRUST_HOTPATCH_AMBIENT",
                    "ANDROID_NDK_HOME\nAPP_FLAVOR\nFRUST_HOTPATCH_AMBIENT\n\
                     FRUST_HOTPATCH_CAPTURE\nFRUST_HOTPATCH_LINK\n\
                     FRUST_HOTPATCH_LINKER\nFRUST_HOTPATCH_LINK_ARGS_FILE\n\
                     FRUST_HOTPATCH_LINK_ERR_FILE\nPATH\nRUSTC_WORKSPACE_WRAPPER"
                ),
            ]
        );
    }

    #[test]
    fn the_fat_image_links_with_identical_code_folding_off() {
        let captured = strings(&[
            "-Wl,--version-script=/t/rustcX/list",
            "/t/deps/app.app.0.rcgu.o",
            "-Wl,--pack-dyn-relocs=android",
            "-Wl,--icf=all",
            "-Wl,--pack-dyn-relocs=android,--icf=safe",
            "-Wl,--icf,all",
            "-o",
            "/t/deps/libapp.so",
            "-shared",
        ]);
        assert_eq!(
            fat_image_link_args(captured),
            strings(&[
                "-Wl,--version-script=/t/rustcX/list",
                "/t/deps/app.app.0.rcgu.o",
                "-Wl,--pack-dyn-relocs=android",
                "-Wl,--pack-dyn-relocs=android",
                "-o",
                "/t/deps/libapp.so",
                "-shared",
                "-Wl,--icf=none",
            ])
        );
    }

    /// A captured Android `cdylib` link line, rustc's shape (see the module
    /// doc's probe): rustc's own temp objects, the tip's `.rcgu.o`, a
    /// workspace rlib under the target dir, a toolchain rlib, the NDK libs,
    /// the Android rustflags' link args.
    fn captured_cdylib_link(target_dir: &Path, workspace_rlib: &Path) -> Vec<String> {
        let deps = target_dir.join("aarch64-linux-android/debug/deps");
        let path = |p: PathBuf| p.to_string_lossy().into_owned();
        vec![
            format!("-Wl,--version-script={}", path(deps.join("rustcX/list"))),
            "-Wl,--no-undefined-version".into(),
            path(deps.join("rustcX/symbols.o")),
            path(deps.join("app.app.0.rcgu.o")),
            "-Wl,--as-needed".into(),
            "-Wl,-Bstatic".into(),
            path(workspace_rlib.to_path_buf()),
            "/rustlib/aarch64-linux-android/lib/libstd-1.rlib".into(),
            "-Wl,-Bdynamic".into(),
            "-ldl".into(),
            "-llog".into(),
            "-lunwind".into(),
            "-lc".into(),
            "-Wl,--eh-frame-hdr".into(),
            "-Wl,-z,noexecstack".into(),
            "-o".into(),
            path(deps.join("libapp.so")),
            "-shared".into(),
            "-Wl,-z,relro,-z,now".into(),
            "-nodefaultlibs".into(),
            "-Wl,--pack-dyn-relocs=android".into(),
            "-Wl,--icf=all".into(),
        ]
    }

    /// A workspace rlib whose one member is a `.rcgu.o`.
    fn workspace_rlib(target_dir: &Path) -> PathBuf {
        let deps = target_dir.join("aarch64-linux-android/debug/deps");
        std::fs::create_dir_all(&deps).unwrap();
        let rlib = deps.join("libcore_lib-1.rlib");
        let member = object(target(), &[Def::Text("core_lib_fn", 8)]);
        let mut builder = ar::Builder::new(Vec::new());
        builder
            .append(
                &ar::Header::new(
                    b"core_lib-1.core_lib.0.rcgu.o".to_vec(),
                    member.len() as u64,
                ),
                member.as_slice(),
            )
            .unwrap();
        std::fs::write(&rlib, builder.into_inner().unwrap()).unwrap();
        rlib
    }

    /// The fat `cdylib`: NDK clang, the captured line with the workspace
    /// rlib's objects force-loaded (`--whole-archive`) right after the last
    /// object, the toolchain rlib kept, folding off, the anchor exported,
    /// the output under the target dir. The runner answers only that exact
    /// argv.
    #[test]
    fn the_fat_cdylib_force_loads_the_archive_and_exports_the_anchor_through_ndk_clang() {
        let dir = temp_dir("fat-link");
        let ndk = fake_ndk(&dir.join("ndk"));
        let target_dir = dir.join("target");
        let rlib = workspace_rlib(&target_dir);
        let link_args = fat_image_link_args(captured_cdylib_link(&target_dir, &rlib));
        let fat_dir = target_dir.join("frust-hotpatch/fat/app-aarch64-linux-android-dev-0");
        let image = fat_dir.join(library_file_name("app"));
        let flavor = LinkerFlavor::for_triple(TRIPLE).unwrap();
        assert_eq!(flavor, LinkerFlavor::Gnu);
        let linker = linker_program(Some(&ndk.clang)).unwrap();
        let archive = fat_link::write_fat_archive(
            &FakeProcessRunner::new(),
            flavor,
            &link_args,
            &target_dir,
            &fat_dir,
        )
        .unwrap()
        .expect("the workspace rlib contributes an object");
        // The linker's output, as NDK clang would leave it.
        std::fs::write(
            &image,
            object(
                target(),
                &[Def::Text("app_fn", 8), Def::Text(ANCHOR_SYMBOL, 4)],
            ),
        )
        .unwrap();

        let deps = target_dir.join("aarch64-linux-android/debug/deps");
        let path = |p: PathBuf| p.to_string_lossy().into_owned();
        let expected: Vec<String> = vec![
            format!("-Wl,--version-script={}", path(deps.join("rustcX/list"))),
            "-Wl,--no-undefined-version".into(),
            path(deps.join("rustcX/symbols.o")),
            path(deps.join("app.app.0.rcgu.o")),
            "-Wl,--whole-archive".into(),
            path(archive.path.clone()),
            "-Wl,--no-whole-archive".into(),
            "/rustlib/aarch64-linux-android/lib/libstd-1.rlib".into(),
            "-Wl,--as-needed".into(),
            "-Wl,-Bstatic".into(),
            "-Wl,-Bdynamic".into(),
            "-ldl".into(),
            "-llog".into(),
            "-lunwind".into(),
            "-lc".into(),
            "-Wl,--eh-frame-hdr".into(),
            "-Wl,-z,noexecstack".into(),
            "-shared".into(),
            "-Wl,-z,relro,-z,now".into(),
            "-nodefaultlibs".into(),
            "-Wl,--pack-dyn-relocs=android".into(),
            "-Wl,--icf=none".into(),
            "-Wl,--export-dynamic-symbol,__frust_hotpatch_anchor".into(),
            "-o".into(),
            path(image.clone()),
        ];
        let runner = FakeProcessRunner::new().with(
            std::iter::once(path(ndk.clang.clone()))
                .chain(expected.iter().cloned())
                .collect::<Vec<_>>()
                .join(" "),
            ok(),
        );
        let out = fat_link::fat_link(
            &runner,
            &FatLinkRequest {
                flavor,
                linker: &linker,
                link_args: &link_args,
                envs: &[],
                target_dir: &target_dir,
                archive_dir: &fat_dir,
                exe: &image,
            },
        )
        .unwrap();
        assert_eq!(linker, path(ndk.clang.clone()));
        assert_eq!(out.exe, image);
        assert_eq!(out.archive.as_ref().map(|a| &a.path), Some(&archive.path));
        assert!(!expected.iter().any(|arg| arg.contains("--icf=all")));
        ensure_symbol_source(&out.exe, &target_dir).unwrap();
    }

    /// A thin patch for Android links through NDK clang with the Gnu thin
    /// arguments (`-shared`, the forwarded NDK libs) and exports the anchor.
    #[test]
    fn thin_links_use_ndk_clang_and_gnu_args_and_export_the_anchor() {
        let dir = temp_dir("thin-link");
        let ndk = fake_ndk(&dir.join("ndk"));
        let target_dir = dir.join("target");
        let deps = target_dir.join("aarch64-linux-android/debug/deps");
        std::fs::create_dir_all(&deps).unwrap();
        let path = |p: PathBuf| p.to_string_lossy().into_owned();
        // The tip lib replay's intercepted link: fresh objects, NDK libs.
        let tip_link_args: Vec<String> = vec![
            path(deps.join("rustcY/symbols.o")),
            path(deps.join("app.app.1.rcgu.o")),
            path(deps.join("app.app.0.rcgu.o")),
            "-ldl".into(),
            "-llog".into(),
            "-lunwind".into(),
            "-lc".into(),
            "-L".into(),
            path(deps.join("rustcY/raw-dylibs")),
            "-o".into(),
            path(deps.join("libapp.so")),
            "-shared".into(),
            "-nodefaultlibs".into(),
        ];
        let session = session_name("app", "emulator-5554");
        let session_dir = target_dir.join("frust-hotpatch").join(&session);
        std::fs::create_dir_all(&session_dir).unwrap();
        let stub = session_dir.join("stub-1.o");
        std::fs::write(&stub, b"stub").unwrap();
        let flavor = LinkerFlavor::for_triple(TRIPLE).unwrap();
        let output = thin_link::patch_path(&target_dir, &session, 1, flavor).unwrap();
        assert_eq!(output, session_dir.join("patch-1.so"));
        std::fs::write(
            &output,
            object(
                target(),
                &[Def::Text("app_fn", 8), Def::Text(ANCHOR_SYMBOL, 4)],
            ),
        )
        .unwrap();
        let rlib = deps.join("libcore_lib-2.rlib");
        let expected: Vec<String> = vec![
            path(deps.join("app.app.0.rcgu.o")),
            path(deps.join("app.app.1.rcgu.o")),
            path(rlib.clone()),
            path(stub.clone()),
            "-shared".into(),
            "-Wl,--eh-frame-hdr".into(),
            "-Wl,-z,noexecstack".into(),
            "-Wl,-z,relro,-z,now".into(),
            "-nodefaultlibs".into(),
            "-Wl,-Bdynamic".into(),
            "-ldl".into(),
            "-llog".into(),
            "-lunwind".into(),
            "-lc".into(),
            "-L".into(),
            path(deps.join("rustcY/raw-dylibs")),
            "-Wl,--export-dynamic-symbol,__frust_hotpatch_anchor".into(),
            format!(
                "-Wl,--version-script={}",
                path(session_dir.join("patch-1.exports"))
            ),
            "-o".into(),
            path(output.clone()),
        ];
        let linker = linker_program(Some(&ndk.clang)).unwrap();
        let runner = FakeProcessRunner::new().with(
            std::iter::once(linker.clone())
                .chain(expected)
                .collect::<Vec<_>>()
                .join(" "),
            ok(),
        );
        let linked = thin_link::thin_link(
            &runner,
            &ThinLinkRequest {
                flavor,
                linker: &linker,
                tip_link_args: &tip_link_args,
                replayed_rlibs: &[rlib],
                stub_object: &stub,
                output: &output,
                envs: &[],
            },
        )
        .unwrap();
        assert_eq!(linked.patch, output);
    }

    /// The symbol cache reads the unstripped fat image under the target
    /// dir; the copy staged into `jniLibs` (which Gradle strips into the
    /// APK) is never a source, even where it still parses.
    #[test]
    fn the_symbol_cache_reads_the_target_dir_image_never_the_packaged_copy() {
        let dir = temp_dir("symbols");
        let target_dir = dir.join("target");
        let fat_dir = target_dir.join("frust-hotpatch/fat/scope");
        std::fs::create_dir_all(&fat_dir).unwrap();
        let image = fat_dir.join(library_file_name("app"));
        let fat = object(
            target(),
            &[
                Def::Text("pad", 32),
                Def::Text("app_fn", 8),
                Def::Text(ANCHOR_SYMBOL, 4),
            ],
        );
        std::fs::write(&image, &fat).unwrap();
        // cargo-ndk's copy of the proxied (thin, non-fat) link.
        let abi_dir = dir.join("app/build/android/jniLibs").join(ABI);
        std::fs::create_dir_all(&abi_dir).unwrap();
        let thin = object(target(), &[Def::Text(ANCHOR_SYMBOL, 4)]);
        std::fs::write(abi_dir.join("libapp.so"), &thin).unwrap();

        let cache = SymbolCache::load(&image, target()).unwrap();
        let staged = stage_fat_image(&image, &abi_dir, &library_file_name("app")).unwrap();
        assert_eq!(staged, abi_dir.join("libapp.so"));
        assert_eq!(std::fs::read(&staged).unwrap(), fat);
        assert!(!abi_dir.join(".libapp.so.partial").exists());

        // Gradle strips the packaged copy; the cache, read from the fat
        // image before staging, is unaffected and still knows `app_fn`.
        std::fs::write(&staged, &thin).unwrap();
        assert_eq!(cache.path(), image.as_path());
        assert!(cache.symbols().get("app_fn").is_some());
        ensure_symbol_source(cache.path(), &target_dir).unwrap();
        for packaged in [
            staged.clone(),
            dir.join("app/build/android/app/outputs/apk/debug/app-debug.apk"),
        ] {
            let err = ensure_symbol_source(&packaged, &target_dir).unwrap_err();
            assert!(
                matches!(err, HotpatchError::BuilderUnsupported { .. }),
                "{err:?}"
            );
        }
    }

    #[test]
    fn keys_carry_the_triple_and_the_cdylib_name() {
        assert_eq!(library_file_name("my_app"), "libmy_app.so");
        assert_eq!(
            session_name("my_app", "0A1B2C3D4E5F"),
            "session-my_app-aarch64-linux-android-0A1B2C3D4E5F"
        );
        assert_ne!(
            session_name("my_app", "0A1B2C3D4E5F"),
            session_name("my_app", "emulator-5554"),
            "two devices never share a session dir"
        );
        let scope = ScopeInputs {
            tip: "my-app".into(),
            triple: TRIPLE.into(),
            profile: "dev".into(),
            features: Vec::new(),
            rustflags: Vec::new(),
            rustc_version: "rustc 1.0".into(),
        };
        let desktop = ScopeInputs {
            triple: "aarch64-apple-darwin".into(),
            ..scope.clone()
        };
        assert!(
            scope
                .dir_name()
                .unwrap()
                .starts_with("my_app-aarch64-linux-android-dev-")
        );
        assert_ne!(scope.dir_name().unwrap(), desktop.dir_name().unwrap());
    }

    /// Desktop session dirs keep their name: `session-<bin>`, no triple and
    /// no serial, so an Android session dir of the same project is another.
    #[test]
    fn the_desktop_session_name_is_unchanged() {
        assert_eq!(session::desktop_session_name("my-app"), "session-my-app");
        assert_ne!(
            session::desktop_session_name("my_app"),
            session_name("my_app", "emulator-5554")
        );
    }

    /// Every serial form adb lists becomes one component `session_dir`
    /// accepts, and distinct serials stay distinct.
    #[test]
    fn serials_become_one_safe_path_component() {
        let long = format!("adb-{}._adb-tls-connect._tcp", "R5CT".repeat(16));
        let serials = [
            "0A1B2C3D4E5F",
            "emulator-5554",
            "192.168.1.5:5555",
            "192-168-1-5-5555",
            "",
            "..",
            "a/b\\c",
            long.as_str(),
        ];
        let target_dir = Path::new("/t");
        let mut seen = std::collections::BTreeSet::new();
        for serial in serials {
            let component = serial_component(serial);
            assert!(!component.is_empty(), "{serial:?}");
            assert!(component.len() <= SERIAL_COMPONENT_MAX, "{component}");
            assert!(
                component
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{component}"
            );
            let session = session_name("app", serial);
            assert_eq!(
                thin_link::session_dir(target_dir, &session).unwrap(),
                target_dir.join("frust-hotpatch").join(&session),
                "{serial:?}"
            );
            assert!(seen.insert(component), "{serial:?} collides");
        }
        assert_eq!(serial_component("0A1B2C3D4E5F"), "0A1B2C3D4E5F");
        assert_eq!(serial_component("emulator-5554"), "emulator-5554");
        assert_eq!(serial_component(""), "no-serial");
        let tcp = serial_component("192.168.1.5:5555");
        assert!(tcp.starts_with("192-168-1-5-5555-"), "{tcp}");
        assert_eq!(tcp.len(), "192-168-1-5-5555-".len() + 8);
        assert_eq!(tcp, serial_component("192.168.1.5:5555"), "stable");
        let cut = serial_component(&long);
        assert_eq!(cut.len(), SERIAL_COMPONENT_MAX);
        assert!(cut.starts_with("adb-R5CT"), "{cut}");
    }

    #[test]
    fn rustflags_fall_back_to_the_triple_scoped_variable() {
        let env = FakeEnv::new().set(
            "CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS",
            "-C link-arg=-Wl,--icf=all",
        );
        assert_eq!(
            android_rustflags(&env),
            strings(&["-C", "link-arg=-Wl,--icf=all"])
        );
        let env = env.set("RUSTFLAGS", "-Cdebuginfo=2");
        assert_eq!(android_rustflags(&env), strings(&["-Cdebuginfo=2"]));
    }

    /// `cargo metadata` for an app package `app` with the given targets.
    fn metadata(targets: serde_json::Value) -> String {
        const APP: &str = "path+file:///w/app#0.1.0";
        serde_json::json!({
            "packages": [{"id": APP, "name": "app", "source": null,
                          "manifest_path": "/w/app/Cargo.toml", "targets": targets}],
            "workspace_members": [APP],
            "resolve": {"nodes": [{"id": APP, "deps": []}], "root": APP},
            "workspace_root": "/w/app",
        })
        .to_string()
    }

    #[test]
    fn the_graph_needs_a_cdylib_lib_and_keys_on_any_bin() {
        let lib = serde_json::json!({"name": "app", "kind": ["cdylib", "staticlib", "rlib"],
            "crate_types": ["cdylib", "staticlib", "rlib"], "src_path": "/w/app/src/lib.rs"});
        let bin = |name: &str| {
            serde_json::json!({"name": name, "kind": ["bin"],
            "crate_types": ["bin"], "src_path": format!("/w/app/src/bin/{name}.rs")})
        };

        let graph = android_graph(
            &metadata(serde_json::json!([lib, bin("zz"), bin("app")])),
            "app",
        )
        .unwrap();
        assert_eq!(
            graph.tip_lib().map(|u| u.record_key().crate_name),
            Some("app".into())
        );

        let rlib_only = serde_json::json!({"name": "app", "kind": ["lib"],
            "crate_types": ["lib"], "src_path": "/w/app/src/lib.rs"});
        let err = android_graph(&metadata(serde_json::json!([rlib_only, bin("app")])), "app")
            .unwrap_err();
        assert!(err.to_string().contains("cdylib"), "{err}");

        let err = android_graph(&metadata(serde_json::json!([lib])), "app").unwrap_err();
        assert!(err.to_string().contains("no bin target"), "{err}");

        let err =
            android_graph(&metadata(serde_json::json!([lib, bin("app")])), "other").unwrap_err();
        assert!(err.to_string().contains("not a workspace member"), "{err}");
    }

    #[test]
    fn a_non_debug_build_is_refused_before_anything_runs() {
        let dir = temp_dir("profile");
        let runner = std::sync::Arc::new(FakeProcessRunner::new());
        let env = FakeEnv::new();
        let host = SessionHost {
            runner: runner.clone(),
            env: &env,
            frust_exe: PathBuf::from("/opt/frust/bin/frust"),
        };
        let info =
            BuildInfo::from_args(crate::build_info::BuildArgs::default(), BuildMode::Profile)
                .unwrap();
        let device = Device {
            id: "emulator-5554".into(),
            name: "Pixel".into(),
            platform: crate::devices::Platform::Android,
            kind: crate::devices::Kind::Emulator,
            os_version: None,
            connection_state: None,
        };
        let start = AndroidStart {
            root: &dir,
            info: &info,
            package: "app",
            device: &device,
        };
        let err = start_android(&host, &start, &mut |_| {}, &AtomicBool::new(false))
            .err()
            .expect("a profile build is never hot");
        assert!(
            matches!(
                err,
                StartError::RestartRequired(RestartReason::BuilderUnsupported { .. })
            ),
            "{err:?}"
        );
        assert!(runner.recorded_cwd().is_none(), "nothing ran");
    }

    #[test]
    fn another_abi_is_refused_before_any_build() {
        let dir = temp_dir("abi");
        let runner = std::sync::Arc::new(FakeProcessRunner::new());
        let env = FakeEnv::new();
        let host = SessionHost {
            runner: runner.clone(),
            env: &env,
            frust_exe: PathBuf::from("/opt/frust/bin/frust"),
        };
        let info = BuildInfo::from_args(crate::build_info::BuildArgs::default(), BuildMode::Debug)
            .unwrap();
        let device = Device {
            id: "emulator-5554".into(),
            name: "Pixel".into(),
            platform: crate::devices::Platform::Android,
            kind: crate::devices::Kind::Emulator,
            os_version: None,
            connection_state: None,
        };
        let start = AndroidStart {
            root: &dir,
            info: &info,
            package: "app",
            device: &device,
        };
        let ndk = fake_ndk(&dir.join("ndk"));
        let err = build_and_stage(&host, &start, &ndk, "x86_64", &mut |_| {})
            .err()
            .expect("x86_64 is refused");
        match err {
            StartError::RestartRequired(RestartReason::BuilderUnsupported { detail }) => {
                assert!(detail.contains("arm64-v8a only"), "{detail}")
            }
            other => panic!("expected BuilderUnsupported, got {other:?}"),
        }
        assert!(runner.recorded_cwd().is_none(), "nothing ran");
    }

    /// Records every `run` invocation (answered by the inner fake), calling
    /// `on_run` with it first: a Ctrl-C landing during that call.
    struct RecordingRunner {
        inner: FakeProcessRunner,
        runs: std::sync::Mutex<Vec<String>>,
        on_run: Box<dyn Fn(&str) + Send + Sync>,
    }

    impl RecordingRunner {
        fn new(inner: FakeProcessRunner) -> Self {
            Self {
                inner,
                runs: std::sync::Mutex::new(Vec::new()),
                on_run: Box::new(|_| {}),
            }
        }

        fn runs(&self) -> Vec<String> {
            self.runs.lock().unwrap().clone()
        }
    }

    impl ProcessRunner for RecordingRunner {
        fn run(&self, cmd: &str, args: &[&str]) -> anyhow::Result<Output> {
            let key = std::iter::once(cmd)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            (self.on_run)(&key);
            self.runs.lock().unwrap().push(key);
            self.inner.run(cmd, args)
        }

        fn run_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
            on_line: &mut dyn FnMut(&str),
        ) -> anyhow::Result<Output> {
            self.inner.run_streaming(cmd, args, cwd, env, on_line)
        }

        fn spawn_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
        ) -> anyhow::Result<crate::process::StreamHandle> {
            self.inner.spawn_streaming(cmd, args, cwd, env)
        }
    }

    const SERIAL: &str = "FAKE-SERIAL";
    const PACKAGE: &str = "it.example.fake";
    const LOGCAT: &str = "adb -s FAKE-SERIAL logcat --pid 4242";
    const FORCE_STOP: &str = "adb -s FAKE-SERIAL shell am force-stop it.example.fake";

    /// The launched app: its logcat stream (hanging after `lines`).
    fn launched(runner: &dyn ProcessRunner) -> AndroidLaunch {
        AndroidLaunch {
            stream: runner.spawn_streaming(LOGCAT, &[], None, &[]).unwrap(),
            package: PACKAGE.to_string(),
        }
    }

    /// A loopback port nothing listens on (bound, then released).
    fn closed_port() -> u16 {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap().port()
    }

    /// Criterion 1: a cancel raised during the discovery wait ends it at
    /// once; the logcat stream is killed and the app force-stopped (no
    /// forward was allocated yet), and the start answers `Ok(None)`.
    #[test]
    fn a_cancel_during_the_discovery_wait_kills_the_stream_and_force_stops_the_app() {
        let runner = RecordingRunner::new(
            FakeProcessRunner::new().with_hanging_stream(LOGCAT, ["I/app: starting"]),
        );
        let mut launch = launched(&runner);
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let raiser = {
            let cancel = std::sync::Arc::clone(&cancel);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                cancel.store(true, Ordering::SeqCst);
            })
        };
        let attached = attach_launched(&runner, SERIAL, &mut launch, &mut |_| {}, &cancel)
            .expect("a cancel is not an error");
        raiser.join().unwrap();
        assert!(attached.is_none());
        assert_eq!(runner.runs(), vec![FORCE_STOP]);
        assert!(
            matches!(
                launch.stream.lines.try_recv(),
                Err(crate::process::TryRecvError::Disconnected)
            ),
            "the logcat stream is killed"
        );
    }

    /// Criterion 1: a cancel landing during the `adb forward` (after the
    /// discovery line) is observed once the attach returns: the forward is
    /// removed and the app force-stopped. Without the cancel the same start
    /// attaches and keeps its forward (the negative control).
    #[test]
    fn a_cancel_during_the_forward_removes_it_and_force_stops_the_app() {
        for cancelled in [true, false] {
            let port = closed_port();
            let discovery = frust_devtools_protocol::format_discovery_line(7777, None);
            let forward = format!("adb -s {SERIAL} forward tcp:0 tcp:7777");
            let mut runner = RecordingRunner::new(
                FakeProcessRunner::new()
                    .with_hanging_stream(LOGCAT, [discovery.as_str()])
                    .with(
                        forward.as_str(),
                        Output {
                            success: true,
                            stdout: format!("{port}\n"),
                            stderr: String::new(),
                        },
                    )
                    .with(format!("adb -s {SERIAL} forward --remove"), ok())
                    .with(FORCE_STOP, ok()),
            );
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            if cancelled {
                let cancel = std::sync::Arc::clone(&cancel);
                let forward = forward.clone();
                runner.on_run = Box::new(move |key| {
                    if key == forward {
                        cancel.store(true, Ordering::SeqCst);
                    }
                });
            }
            let mut launch = launched(&runner);
            let attached = attach_launched(&runner, SERIAL, &mut launch, &mut |_| {}, &cancel)
                .expect("no launch error");
            if cancelled {
                assert!(attached.is_none());
                assert_eq!(
                    runner.runs(),
                    vec![
                        forward.clone(),
                        format!("adb -s {SERIAL} forward --remove tcp:{port}"),
                        FORCE_STOP.to_string(),
                    ]
                );
            } else {
                let (_app, forward_port) = attached.expect("attached");
                assert_eq!(forward_port, Some(port));
                assert_eq!(runner.runs(), vec![forward.clone()]);
                launch.stream.kill();
            }
        }
    }

    /// Criterion 1 (hot start): with `cancel` raised, a pipeline that issued
    /// `am start` and then answered a failure (the Ctrl-C's SIGINT also
    /// killed the in-flight `adb shell am start`) or a cancel force-stops
    /// the package it launched. Without the cancel a failure stops nothing
    /// (the negative control), and a run stopped before `am start` names no
    /// package.
    #[test]
    fn a_cancelled_hot_start_force_stops_what_the_pipeline_launched_on_a_failure_too() {
        let am_start_failed = || Some(anyhow::anyhow!("`adb shell am start` failed: killed"));
        for (raised, error, launched, stops) in [
            (true, am_start_failed(), Some(PACKAGE), true),
            (true, None, Some(PACKAGE), true),
            (false, am_start_failed(), Some(PACKAGE), false),
            (true, None, None, false),
        ] {
            let failed = error.is_some();
            let runner = RecordingRunner::new(FakeProcessRunner::new().with(FORCE_STOP, ok()));
            let settled = settle_pipeline(
                &runner,
                SERIAL,
                Err(StoppedShort {
                    error,
                    launched: launched.map(str::to_string),
                }),
                None,
                &AtomicBool::new(raised),
            );
            let expected: Vec<String> = if stops {
                vec![FORCE_STOP.to_string()]
            } else {
                Vec::new()
            };
            assert_eq!(runner.runs(), expected, "raised {raised}, failed {failed}");
            match settled {
                Err(StartError::Launch { detail }) => {
                    assert!(failed, "{detail}");
                    assert!(detail.contains("am start"), "{detail}");
                }
                Ok(None) => assert!(!failed),
                Err(other) => panic!("unexpected {other:?}"),
                Ok(Some(_)) => panic!("nothing was streamed"),
            }
        }
    }
}
