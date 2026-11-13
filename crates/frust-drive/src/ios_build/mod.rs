//! iOS release build pipeline: `frust build ios` (device or simulator
//! `.app`) and `frust build ipa` (archive + `-exportArchive`).
//!
//! The [`build`] signature and the [`IosArtifact`]/[`BuiltArtifacts`] types
//! are load-bearing for `commands::build` and stay frozen; this module owns
//! the `xcodebuild`-driving body — scheme/configuration resolution
//! ([`schemes`]), `DEVELOPMENT_TEAM` auto-detect ([`team`]), argv assembly
//! ([`xcodebuild`]), and exportOptions plist generation ([`export`]).

mod export;
mod schemes;
// `pub(crate)`: `doctor::report`'s Signing component reuses `team::detect`
// rather than re-implementing `security find-identity` parsing.
pub(crate) mod team;
pub(crate) mod xcodebuild;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::build_info::{BuildInfo, BuildMode};
use crate::doctor::{EnvLookup, RealEnv};
use crate::facade_key::FacadeKey;
use crate::identity::{self, AppIdentity, IdentityPlatform};
use crate::process::{Output, ProcessRunner, tail_lines};

use self::xcodebuild::{Invocation, Signing};

/// The artifact `frust build ios`/`ipa` requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IosArtifact {
    /// A `.app` build for a device or the Simulator.
    App { simulator: bool, codesign: bool },
    /// An archived + exported `.ipa` for the given export method
    /// (`app-store-connect`/`release-testing`/`debugging`/`enterprise`).
    Ipa { export_method: String },
}

/// Paths to the artifact(s) a successful build produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuiltArtifacts {
    pub paths: Vec<PathBuf>,
}

/// Drives the iOS release pipeline (`xcodebuild build`/`archive` +
/// `-exportArchive`) for `target` in the Frust project rooted at
/// `project_dir`.
///
/// **Print-free core.** Every line this pipeline would surface (streamed
/// `[xcodebuild] …` output, the team-resolution note) is emitted through
/// `on_line`, never `println!` — so a caller holding a raw-mode terminal (the
/// `frust-tui` build session, or the physical-device run session in
/// `ios_run`) can route it into a log tab instead of leaking it to the tty
/// (an inherited/`println!`-ed `xcodebuild` env dump garbles a raw-mode
/// terminal). The CLI (`commands::build`) passes an `on_line` that just
/// `println!`s each line, preserving its stdout verbatim.
///
/// `extra_features` is the front-end's `--features` passthrough, appended to
/// the mode's own cargo features and carried down to `cargo` inside the same
/// base64 `FRUST_FEATURES` build setting the mode's features ride (see
/// [`encode_features`]); an empty slice reproduces the pre-passthrough
/// invocation exactly. `frust-tui`'s build session has no flag surface for it
/// and passes an empty slice.
///
/// **Frozen signature** — do not change it without updating every caller
/// (`commands::build`, `frust-tui`'s build session, `ios_run`'s physical-run
/// session) and this doc comment. The `on_line` sink was added by the
/// tty-garbling fix; `extra_features` by the cargo-feature passthrough.
pub fn build(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    target: &IosArtifact,
    extra_features: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<BuiltArtifacts> {
    build_with_env(
        runner,
        &RealEnv,
        project_dir,
        info,
        target,
        extra_features,
        on_line,
    )
}

/// The team-resolution-testable core: takes an injected [`EnvLookup`] so
/// `FRUST_IOS_TEAM` precedence can be exercised without mutating the real
/// process environment.
fn build_with_env(
    runner: &dyn ProcessRunner,
    env: &dyn EnvLookup,
    project_dir: &Path,
    info: &BuildInfo,
    target: &IosArtifact,
    extra_features: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<BuiltArtifacts> {
    let simulator = matches!(
        target,
        IosArtifact::App {
            simulator: true,
            ..
        }
    );

    preflight(runner, simulator).map_err(|err| anyhow::anyhow!(err))?;

    let manifest = crate::manifest::load_optional(project_dir)?;
    let identity = resolve_identity(manifest.as_ref(), info)?;
    let flavor_declared = identity.as_ref().is_some_and(|i| i.flavor_declared);
    let sc = schemes::resolve_verified(runner, project_dir, info, flavor_declared)?;
    let resolved = crate::build_info::resolve_version(info, manifest.as_ref());

    let defines_b64 = encode_defines(&info.defines);
    let features_b64 =
        resolve_features_b64_preflighted(runner, project_dir, info.mode, extra_features, on_line)?;

    match target {
        IosArtifact::App {
            simulator,
            codesign,
        } => {
            let signing = if *simulator {
                Signing::Simulator
            } else if *codesign {
                let choice = team::resolve(env, runner, project_dir)?;
                if let Some(note) = &choice.note {
                    on_line(note);
                }
                Signing::Automatic { team: choice.team }
            } else {
                Signing::NoCodesign
            };

            let inv = Invocation {
                scheme: &sc.scheme,
                configuration: &sc.configuration,
                simulator: *simulator,
                signing,
                marketing_version: resolved.name.as_deref(),
                current_project_version: resolved.number,
                defines_b64: defines_b64.as_deref(),
                features_b64: features_b64.as_deref(),
                bundle_identifier: None,
                display_name: None,
            }
            .with_identity(identity.as_ref());
            run_xcodebuild(runner, project_dir, info.mode, &inv.build_argv(), on_line)?;

            let products = products_dir(project_dir, &sc.configuration, *simulator);
            let app = glob_one(&products, "app").with_context(|| {
                format!("locating the built `.app` under `{}`", products.display())
            })?;
            check_built_identity(runner, &app, identity.as_ref(), on_line)?;
            Ok(BuiltArtifacts { paths: vec![app] })
        }
        IosArtifact::Ipa { export_method } => {
            // `Ipa` carries no codesign flag by construction — signing is
            // always required for an archive/export.
            let choice = team::resolve(env, runner, project_dir)?;
            if let Some(note) = &choice.note {
                on_line(note);
            }

            if info.mode == BuildMode::Release && resolved.is_defaulted() {
                // An unset value is not overridden, so the project's
                // pbxproj applies.
                on_line(&crate::build_info::ResolvedVersion::native_unset_warning());
            }

            let inv = Invocation {
                scheme: &sc.scheme,
                configuration: &sc.configuration,
                simulator: false,
                signing: Signing::Automatic {
                    team: choice.team.clone(),
                },
                marketing_version: resolved.name.as_deref(),
                current_project_version: resolved.number,
                defines_b64: defines_b64.as_deref(),
                features_b64: features_b64.as_deref(),
                bundle_identifier: None,
                display_name: None,
            }
            .with_identity(identity.as_ref());
            run_xcodebuild(runner, project_dir, info.mode, &inv.archive_argv(), on_line)?;
            // The archived `.app`; a layout without one is left to the
            // export step to report.
            if let Ok(app) = glob_one(&project_dir.join(ARCHIVED_APPS_DIR), "app") {
                check_built_identity(runner, &app, identity.as_ref(), on_line)?;
            }

            let plist = export::export_options_plist(export_method, Some(&choice.team));
            let plist_path = project_dir.join(export::EXPORT_OPTIONS_PATH);
            if let Some(parent) = plist_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating `{}`", parent.display()))?;
            }
            fs::write(&plist_path, plist)
                .with_context(|| format!("writing `{}`", plist_path.display()))?;

            run_xcodebuild(
                runner,
                project_dir,
                info.mode,
                &export::export_argv(),
                on_line,
            )?;

            let ipa_dir = project_dir.join(export::EXPORT_PATH);
            let ipa = glob_one(&ipa_dir, "ipa").with_context(|| {
                format!("locating the exported `.ipa` under `{}`", ipa_dir.display())
            })?;
            Ok(BuiltArtifacts { paths: vec![ipa] })
        }
    }
}

/// Where an archive keeps its `.app`, relative to the project root.
const ARCHIVED_APPS_DIR: &str = "build/ios/archive/Runner.xcarchive/Products/Applications";

/// The iOS identity a build of `info`'s flavor must produce, resolved from
/// the project's `frust.toml`; `None` when the project has no manifest.
fn resolve_identity(
    manifest: Option<&crate::manifest::Manifest>,
    info: &BuildInfo,
) -> Result<Option<AppIdentity>> {
    manifest
        .map(|manifest| identity::resolve(manifest, IdentityPlatform::Ios, info.flavor.as_deref()))
        .transpose()
        .context("resolving the iOS bundle identifier from frust.toml")
}

/// The post-build identity check both iOS lanes run on the `.app` they
/// built, for an explicit `identity` only (a derived one passes no override,
/// so the project's own values are what it should carry):
///
/// - a `CFBundleIdentifier` other than the resolved identifier is an error
///   naming both (the template's `Info.plist` reads it from
///   `PRODUCT_BUNDLE_IDENTIFIER`, which the build sets);
/// - a `CFBundleDisplayName` other than the resolved display name is a
///   `warning:` through `on_line` carrying the two template lines that let
///   `FRUST_DISPLAY_NAME` reach it (a project scaffolded before the setting
///   existed has a literal name there).
///
/// A key `plutil` cannot read is not checked: the physical run lane's own
/// launch-identity read ([`crate::ios_run::bundle_id`]) warns about that.
pub(crate) fn check_built_identity(
    runner: &dyn ProcessRunner,
    app: &Path,
    identity: Option<&AppIdentity>,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    let Some(identity) = identity.filter(|identity| identity.explicit) else {
        return Ok(());
    };
    let plist = app.join("Info.plist");
    if let Some(built) = read_plist_string(runner, &plist, "CFBundleIdentifier")
        && built != identity.identifier
    {
        bail!("{}", identifier_mismatch(&built, &identity.identifier));
    }
    if let Some(built) = read_plist_string(runner, &plist, "CFBundleDisplayName")
        && built != identity.display_name
    {
        on_line(&display_name_warning(&built, &identity.display_name));
    }
    Ok(())
}

/// The error for a built `CFBundleIdentifier` that is not the resolved one.
fn identifier_mismatch(built: &str, expected: &str) -> String {
    format!(
        "the built app's bundle identifier `{built}` does not match `{expected}`, the one \
         resolved from frust.toml. Check that `CFBundleIdentifier` in ios/Runner/Info.plist is \
         `$(PRODUCT_BUNDLE_IDENTIFIER)`."
    )
}

/// The one-line migration warning for a built `CFBundleDisplayName` that is
/// not the resolved one: the template's two lines, the pbxproj one keeping
/// the name the app shows today as its no-override default.
fn display_name_warning(built: &str, expected: &str) -> String {
    format!(
        "warning: the built app's display name `{built}` is not `{expected}`, the one resolved \
         from frust.toml. To let frust set it, make `CFBundleDisplayName` in \
         ios/Runner/Info.plist `<string>$(FRUST_DISPLAY_NAME)</string>` and add \
         `FRUST_DISPLAY_NAME = \"{built}\";` beside `PRODUCT_BUNDLE_IDENTIFIER` in each of the \
         Runner target's build configurations in ios/Runner.xcodeproj/project.pbxproj."
    )
}

/// `plutil -extract <key> raw <plist>`'s first non-empty line, or `None`
/// when `plutil` cannot run, exits non-zero, or prints nothing.
fn read_plist_string(runner: &dyn ProcessRunner, plist: &Path, key: &str) -> Option<String> {
    let plist = plist.to_string_lossy();
    let out = runner
        .run("plutil", &["-extract", key, "raw", &plist])
        .ok()?;
    if !out.success {
        return None;
    }
    out.stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Minimal build preflight (mirrors `ios_run::preflight`'s Xcode/Rust-target
/// checks; the macOS-host gate is already applied by `commands::build`, and a
/// device/simulator udid is not relevant to a build).
fn preflight(runner: &dyn ProcessRunner, simulator: bool) -> Result<(), String> {
    match runner.run("xcode-select", &["-p"]) {
        Ok(out) if out.success => {}
        _ => return Err("Xcode not found. Run: xcode-select --install".to_string()),
    }
    let target = if simulator {
        "aarch64-apple-ios-sim"
    } else {
        "aarch64-apple-ios"
    };
    match runner.run("rustup", &["target", "list", "--installed"]) {
        Ok(out) if out.success && out.stdout.lines().any(|line| line.trim() == target) => {}
        _ => {
            return Err(format!(
                "missing Rust target {target}. Run: rustup target add {target}"
            ));
        }
    }
    Ok(())
}

/// The base64 `FRUST_FEATURES` value for a `mode` build of the app at
/// `project_dir` with the `extra_features` passthrough — `None` for no
/// features. The one resolution both iOS lanes (this module's `build` and
/// `ios_run`'s simulator session) share:
///
/// - **Release-lean preflight:** a legacy app that predates the `lean`
///   feature has it dropped (with a one-time warning through `on_line`) so
///   xcodebuild never threads an undeclared `--features lean` down to
///   `cargo` — cargo's opaque hard error. A declaring app keeps byte-identical
///   features and warns nothing.
/// - **Facade key:** the template run-script hands the decoded CSV to
///   `cargo build --features` verbatim, so each canonical `frust/<feature>`
///   is spelled with the app's facade dependency key ([`FacadeKey`], resolved
///   once from `project_dir`) — `frust-ui/devtools` for an app depending on
///   `frust-ui = ".."`. A manifest naming the facade under two keys is an
///   error here, before xcodebuild runs.
///
/// Pure host-side logic (no Xcode), so it is unit-tested on every host.
#[cfg(test)]
pub(crate) fn resolve_features_b64(
    project_dir: &Path,
    mode: BuildMode,
    extra_features: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<Option<String>> {
    let features = resolve_features(project_dir, mode, extra_features, on_line)?;
    let feature_refs: Vec<&str> = features.iter().map(String::as_str).collect();
    Ok(encode_features(&feature_refs))
}

/// The facade-key-spelled cargo features for `mode` (see
/// [`resolve_features_b64`] for the release-lean and key steps).
fn resolve_features(
    project_dir: &Path,
    mode: BuildMode,
    extra_features: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<Vec<String>> {
    let (features, warning) =
        crate::cargo_manifest::resolve_release_features(project_dir, mode, extra_features);
    if let Some(warning) = warning {
        on_line(&warning);
    }
    let key = FacadeKey::resolve(project_dir).context("resolving the app's frust dependency")?;
    Ok(features.iter().map(|feature| key.map(feature)).collect())
}

/// [`resolve_features_b64`] after resolving the app's dependency graph under
/// those features in this process: a Debug/Profile build's features pull in
/// crates the app's lockfile may lack, and a stale index entry should surface
/// as cargo's own text rather than from inside xcodebuild's run-script.
/// Release adds no crate, so it skips the resolve. The one entry point both
/// the `frust build` lane and the simulator run pipeline share.
pub(crate) fn resolve_features_b64_preflighted(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    mode: BuildMode,
    extra_features: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<Option<String>> {
    let features = resolve_features(project_dir, mode, extra_features, on_line)?;
    if mode != BuildMode::Release {
        crate::lock_refresh::ensure_resolves(runner, project_dir, &features, on_line)
            .map_err(|err| anyhow::anyhow!(err))?;
    }
    let feature_refs: Vec<&str> = features.iter().map(String::as_str).collect();
    Ok(encode_features(&feature_refs))
}

/// Streams an `xcrun xcodebuild …` invocation, prefixing each line, and turns
/// a non-zero exit into an error whose message tails both stdout and stderr
/// (xcodebuild reports most errors on stdout) plus a `frust doctor` hint —
/// signing errors pass through verbatim.
fn run_xcodebuild(
    runner: &dyn ProcessRunner,
    root: &Path,
    mode: BuildMode,
    argv: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let mut prefixed = |line: &str| on_line(&format!("[xcodebuild] {line}"));
    let out = runner
        .run_streaming("xcrun", &refs, Some(root), &[], &mut prefixed)
        .with_context(|| format!("running `xcodebuild` in `{}`", root.display()))?;
    if !out.success {
        bail!("{}", failure_message(&out, mode));
    }
    Ok(())
}

/// A failed xcodebuild's report: both output tails, the dependency-key hint
/// when a `mode` build's cargo step named a feature the package does not
/// contain ([`crate::desktop_run::facade_feature_hint`]), then the doctor
/// hint.
fn failure_message(out: &Output, mode: BuildMode) -> String {
    let stdout_tail = tail_lines(&out.stdout, 50);
    let stderr_tail = tail_lines(&out.stderr, 50);
    let mut message = "`xcodebuild` failed".to_string();
    if !stdout_tail.is_empty() {
        message.push_str(&format!("\n--- stdout (tail) ---\n{stdout_tail}"));
    }
    if !stderr_tail.is_empty() {
        message.push_str(&format!("\n--- stderr (tail) ---\n{stderr_tail}"));
    }
    if let Some(hint) =
        crate::desktop_run::facade_feature_hint(mode, &format!("{}\n{}", out.stdout, out.stderr))
    {
        message.push('\n');
        message.push_str(hint);
    }
    message.push_str("\nhint: run `frust doctor` to check your Xcode/signing setup.");
    message
}

/// The `-derivedDataPath build/ios` products directory for a configuration.
fn products_dir(root: &Path, configuration: &str, simulator: bool) -> PathBuf {
    let sdk = if simulator {
        "iphonesimulator"
    } else {
        "iphoneos"
    };
    root.join("build/ios/Build/Products")
        .join(format!("{configuration}-{sdk}"))
}

/// Returns the single (lexicographically first) entry in `dir` with extension
/// `ext`, erroring if the directory is unreadable or holds no such artifact.
fn glob_one(dir: &Path, ext: &str) -> Result<PathBuf> {
    let entries = fs::read_dir(dir).with_context(|| format!("reading `{}`", dir.display()))?;
    let mut matches: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some(ext))
        .collect();
    matches.sort();
    matches
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no `*.{ext}` artifact found in `{}`", dir.display()))
}

/// base64-encodes `--define`s as `K=V;K=V` (keys sorted for determinism) for
/// the `FRUST_DEFINES` build setting, matching the template run-script's
/// `base64 -d | tr ';' ' '` decode. `None` when there are no defines.
fn encode_defines(defines: &HashMap<String, String>) -> Option<String> {
    if defines.is_empty() {
        return None;
    }
    let mut pairs: Vec<(&String, &String)> = defines.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let joined = pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(";");
    Some(base64_encode(joined.as_bytes()))
}

/// base64-encodes the resolved cargo `features` as `feat,feat` for the
/// `FRUST_FEATURES` build setting, matching the template run-script's
/// `base64 -d` decode into `--features <csv>`.
/// `None` when there are no features to pass — the release-lean preflight can
/// legitimately produce an empty list for a legacy app whose `lean` feature
/// was dropped (`cargo_manifest::resolve_release_features`), in which case no
/// `FRUST_FEATURES=` setting is emitted at all. Shared with the simulator run
/// path (`ios_run::xcodebuild`) so the feature encoding has one source.
/// `features` arrive already spelled with the app's facade key
/// ([`resolve_features_b64`]); this encodes what it is given.
pub(crate) fn encode_features(features: &[&str]) -> Option<String> {
    if features.is_empty() {
        return None;
    }
    Some(base64_encode(features.join(",").as_bytes()))
}

/// Standard base64 (with `=` padding) — a small hand-rolled encoder so this
/// crate needs no `base64` dependency for one build setting.
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::BuildMode;
    use crate::doctor::FakeEnv;
    use crate::process::FakeProcessRunner;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// No `--features` passthrough — byte-identical to the pre-passthrough
    /// invocation, which is what every case but an explicit passthrough test
    /// asserts against.
    const NO_EXTRA: &[String] = &[];

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    const LIST_JSON: &str = r#"{"project":{"name":"Runner","schemes":["Runner"],"configurations":["Debug","Profile","Release"]}}"#;

    /// Registers the preflight + `-list` fixtures every pipeline needs.
    fn base_runner() -> FakeProcessRunner {
        FakeProcessRunner::new()
            .with("cargo metadata --format-version 1", ok("{}"))
            .with("xcode-select -p", ok("/Applications/Xcode.app\n"))
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios\naarch64-apple-ios-sim\n"),
            )
            .with(
                "xcrun xcodebuild -list -json -project ios/Runner.xcodeproj",
                ok(LIST_JSON),
            )
    }

    fn temp_project(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-ios-build-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("ios/Runner.xcodeproj")).unwrap();
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n",
        )
        .unwrap();
        dir
    }

    fn plant_products(dir: &Path, configuration: &str, simulator: bool, name: &str) {
        let products = products_dir(dir, configuration, simulator);
        fs::create_dir_all(products.join(name)).unwrap();
    }

    fn info(mode: BuildMode) -> BuildInfo {
        BuildInfo {
            mode,
            flavor: None,
            defines: HashMap::new(),
            build_name: None,
            build_number: None,
        }
    }

    #[test]
    fn encode_defines_is_sorted_and_base64() {
        let mut defines = HashMap::new();
        defines.insert("B".to_string(), "2".to_string());
        defines.insert("A".to_string(), "1".to_string());
        // "A=1;B=2" -> base64.
        assert_eq!(encode_defines(&defines).as_deref(), Some("QT0xO0I9Mg=="));
        assert_eq!(encode_defines(&HashMap::new()), None);
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    }

    #[test]
    fn build_ios_device_signed_uses_expected_argv_and_reports_app() {
        let dir = temp_project("device-signed");
        plant_products(&dir, "Release", false, "Runner.app");
        let runner = base_runner()
            .with(
                "security find-identity -v -p codesigning",
                ok(r#"  1) H "Apple Development: Ada (TEAMID1234)""#),
            )
            .with(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=bGVhbg== DEVELOPMENT_TEAM=TEAMID1234 CODE_SIGN_STYLE=Automatic CODE_SIGNING_ALLOWED=YES CODE_SIGNING_REQUIRED=YES -allowProvisioningUpdates -allowProvisioningDeviceRegistration build",
                ok("Build succeeded"),
            );
        let env = FakeEnv::new();
        let artifacts = build_with_env(
            &runner,
            &env,
            &dir,
            &info(BuildMode::Release),
            &IosArtifact::App {
                simulator: false,
                codesign: true,
            },
            NO_EXTRA,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(
            artifacts.paths,
            vec![products_dir(&dir, "Release", false).join("Runner.app")]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_ios_device_no_codesign_skips_team_detection() {
        let dir = temp_project("device-nocodesign");
        plant_products(&dir, "Release", false, "Runner.app");
        // No `security find-identity` fixture registered: if team detection
        // ran, the invocation would error via the missing-binary path.
        let runner = base_runner().with(
            "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=bGVhbg== CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY= build",
            ok("Build succeeded"),
        );
        let env = FakeEnv::new();
        let artifacts = build_with_env(
            &runner,
            &env,
            &dir,
            &info(BuildMode::Release),
            &IosArtifact::App {
                simulator: false,
                codesign: false,
            },
            NO_EXTRA,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(artifacts.paths.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Legacy direction: a `--release` build against an app whose Cargo.toml
    /// declares no `lean` feature drops it and warns once through `on_line`;
    /// the xcodebuild invocation carries NO `FRUST_FEATURES=` build setting
    /// (the fixture omits it), so a regression that kept `lean` would
    /// surface via the absent warning. No-codesign device build for simplicity.
    #[test]
    fn release_legacy_app_drops_lean_and_warns() {
        let dir = temp_project("f2-legacy");
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
        plant_products(&dir, "Release", false, "Runner.app");
        let runner = base_runner().with(
            "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY= build",
            ok("Build succeeded"),
        );
        let env = FakeEnv::new();
        let mut lines = Vec::new();
        let artifacts = build_with_env(
            &runner,
            &env,
            &dir,
            &info(BuildMode::Release),
            &IosArtifact::App {
                simulator: false,
                codesign: false,
            },
            NO_EXTRA,
            &mut |l| lines.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(artifacts.paths.len(), 1);
        assert!(
            lines.iter().any(|l| l.contains("lean")),
            "legacy release build must warn about the missing `lean` feature: {lines:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Declaring direction: an app that declares `lean` keeps it —
    /// `FRUST_FEATURES=bGVhbg==` (base64 "lean"), registered exactly, so a
    /// regression that dropped it would produce a shorter, non-matching argv
    /// and error early — and warns nothing.
    #[test]
    fn release_declaring_app_keeps_lean_without_warning() {
        let dir = temp_project("f2-declaring");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\n\n[features]\nlean = [\"log/release_max_level_warn\"]\n",
        )
        .unwrap();
        plant_products(&dir, "Release", false, "Runner.app");
        let runner = base_runner().with(
            "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=bGVhbg== CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY= build",
            ok("Build succeeded"),
        );
        let env = FakeEnv::new();
        let mut lines = Vec::new();
        let artifacts = build_with_env(
            &runner,
            &env,
            &dir,
            &info(BuildMode::Release),
            &IosArtifact::App {
                simulator: false,
                codesign: false,
            },
            NO_EXTRA,
            &mut |l| lines.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(artifacts.paths.len(), 1);
        assert!(
            !lines.iter().any(|l| l.contains("lean")),
            "a declaring app must not warn: {lines:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The `--features` passthrough reaches `cargo` through the SAME base64
    /// `FRUST_FEATURES` build setting the mode's own features ride, appended
    /// after them. Registered exactly (base64 "lean,hybrid-tier"), so a
    /// replaced selection or a reordered CSV produces a non-matching argv and
    /// errors instead of silently passing.
    #[test]
    fn passthrough_features_ride_the_frust_features_setting_after_the_modes_own() {
        let dir = temp_project("passthrough-ios");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\n\n[features]\nlean = [\"log/release_max_level_warn\"]\n",
        )
        .unwrap();
        plant_products(&dir, "Release", false, "Runner.app");
        let runner = base_runner().with(
            "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=bGVhbixoeWJyaWQtdGllcg== CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY= build",
            ok("Build succeeded"),
        );
        let env = FakeEnv::new();
        let artifacts = build_with_env(
            &runner,
            &env,
            &dir,
            &info(BuildMode::Release),
            &IosArtifact::App {
                simulator: false,
                codesign: false,
            },
            &["hybrid-tier".to_string()],
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(artifacts.paths.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_ios_simulator_uses_simulator_argv() {
        let dir = temp_project("simulator");
        plant_products(&dir, "Release", true, "Runner.app");
        let runner = base_runner().with(
            format!("xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphonesimulator -destination generic/platform=iOS Simulator -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=bGVhbg== build", xcodebuild::host_sim_arch()),
            ok("Build succeeded"),
        );
        let env = FakeEnv::new();
        let artifacts = build_with_env(
            &runner,
            &env,
            &dir,
            &info(BuildMode::Release),
            &IosArtifact::App {
                simulator: true,
                codesign: false,
            },
            NO_EXTRA,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(
            artifacts.paths,
            vec![products_dir(&dir, "Release", true).join("Runner.app")]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_ios_device_signed_passes_version_overrides_when_flagged() {
        let dir = temp_project("device-versions");
        plant_products(&dir, "Release", false, "Runner.app");
        let runner = base_runner()
            .with(
                "security find-identity -v -p codesigning",
                ok(r#"  1) H "Apple Development: Ada (TEAMID1234)""#),
            )
            .with(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios MARKETING_VERSION=2.0.1 CURRENT_PROJECT_VERSION=7 FRUST_FEATURES=bGVhbg== DEVELOPMENT_TEAM=TEAMID1234 CODE_SIGN_STYLE=Automatic CODE_SIGNING_ALLOWED=YES CODE_SIGNING_REQUIRED=YES -allowProvisioningUpdates -allowProvisioningDeviceRegistration build",
                ok("Build succeeded"),
            );
        let env = FakeEnv::new();
        let mut info = info(BuildMode::Release);
        info.build_name = Some("2.0.1".to_string());
        info.build_number = Some(7);
        let artifacts = build_with_env(
            &runner,
            &env,
            &dir,
            &info,
            &IosArtifact::App {
                simulator: false,
                codesign: true,
            },
            NO_EXTRA,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(artifacts.paths.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_ipa_archives_writes_plist_and_exports() {
        let dir = temp_project("ipa");
        // Plant the exported ipa the final glob will find.
        fs::create_dir_all(dir.join("build/ios/ipa")).unwrap();
        fs::write(dir.join("build/ios/ipa/Runner.ipa"), b"fake").unwrap();

        let runner = base_runner()
            .with(
                "security find-identity -v -p codesigning",
                ok(r#"  1) H "Apple Development: Ada (TEAMID1234)""#),
            )
            .with(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=bGVhbg== DEVELOPMENT_TEAM=TEAMID1234 CODE_SIGN_STYLE=Automatic CODE_SIGNING_ALLOWED=YES CODE_SIGNING_REQUIRED=YES -allowProvisioningUpdates -allowProvisioningDeviceRegistration -archivePath build/ios/archive/Runner.xcarchive archive",
                ok("Archive succeeded"),
            )
            .with(
                "xcrun xcodebuild -exportArchive -archivePath build/ios/archive/Runner.xcarchive -exportOptionsPlist build/ios/ExportOptions.plist -exportPath build/ios/ipa -allowProvisioningUpdates",
                ok("Export succeeded"),
            );
        let env = FakeEnv::new();
        let artifacts = build_with_env(
            &runner,
            &env,
            &dir,
            &info(BuildMode::Release),
            &IosArtifact::Ipa {
                export_method: "app-store-connect".to_string(),
            },
            NO_EXTRA,
            &mut |_| {},
        )
        .unwrap();

        assert_eq!(artifacts.paths, vec![dir.join("build/ios/ipa/Runner.ipa")]);
        let plist = fs::read_to_string(dir.join("build/ios/ExportOptions.plist")).unwrap();
        assert!(
            plist.contains("<key>method</key><string>app-store-connect</string>"),
            "{plist}"
        );
        assert!(
            plist.contains("<key>teamID</key><string>TEAMID1234</string>"),
            "{plist}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Runs a release ipa build in a project whose `frust.toml` carries
    /// `app_extra`, with `info`, expecting the archive argv to carry
    /// `version_args`; returns every line sent through `on_line`.
    fn ipa_lines(tag: &str, app_extra: &str, info: &BuildInfo, version_args: &str) -> Vec<String> {
        let dir = temp_project(tag);
        fs::write(
            dir.join("frust.toml"),
            format!("[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n{app_extra}"),
        )
        .unwrap();
        fs::create_dir_all(dir.join("build/ios/ipa")).unwrap();
        fs::write(dir.join("build/ios/ipa/Runner.ipa"), b"fake").unwrap();
        let runner = base_runner()
            .with(
                "security find-identity -v -p codesigning",
                ok(r#"  1) H "Apple Development: Ada (TEAMID1234)""#),
            )
            .with(
                format!("xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios {version_args}FRUST_FEATURES=bGVhbg== DEVELOPMENT_TEAM=TEAMID1234 CODE_SIGN_STYLE=Automatic CODE_SIGNING_ALLOWED=YES CODE_SIGNING_REQUIRED=YES -allowProvisioningUpdates -allowProvisioningDeviceRegistration -archivePath build/ios/archive/Runner.xcarchive archive"),
                ok("Archive succeeded"),
            )
            .with(
                "xcrun xcodebuild -exportArchive -archivePath build/ios/archive/Runner.xcarchive -exportOptionsPlist build/ios/ExportOptions.plist -exportPath build/ios/ipa -allowProvisioningUpdates",
                ok("Export succeeded"),
            );
        let mut lines = Vec::new();
        build_with_env(
            &runner,
            &FakeEnv::new(),
            &dir,
            info,
            &IosArtifact::Ipa {
                export_method: "app-store-connect".to_string(),
            },
            NO_EXTRA,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        let _ = fs::remove_dir_all(&dir);
        lines
    }

    fn version_warnings(lines: &[String]) -> Vec<&String> {
        lines
            .iter()
            .filter(|l| l.starts_with("warning: app version not set"))
            .collect()
    }

    #[test]
    fn ipa_takes_the_manifest_version_without_a_warning() {
        let lines = ipa_lines(
            "ipa-manifest-version",
            "version = \"3.4.5\"\nbuild-number = 12\n",
            &info(BuildMode::Release),
            "MARKETING_VERSION=3.4.5 CURRENT_PROJECT_VERSION=12 ",
        );
        assert!(version_warnings(&lines).is_empty(), "{lines:?}");
    }

    #[test]
    fn ipa_flag_beats_the_manifest_version() {
        let mut flagged = info(BuildMode::Release);
        flagged.build_name = Some("9.9.9".to_string());
        flagged.build_number = Some(99);
        let lines = ipa_lines(
            "ipa-flag-version",
            "version = \"3.4.5\"\nbuild-number = 12\n",
            &flagged,
            "MARKETING_VERSION=9.9.9 CURRENT_PROJECT_VERSION=99 ",
        );
        assert!(version_warnings(&lines).is_empty(), "{lines:?}");
    }

    #[test]
    fn ipa_with_a_defaulted_version_warns_exactly_once() {
        let lines = ipa_lines("ipa-default-version", "", &info(BuildMode::Release), "");
        assert_eq!(
            version_warnings(&lines),
            vec![&crate::build_info::ResolvedVersion::native_unset_warning()],
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains("(1)")), "{lines:?}");
    }

    #[test]
    fn build_ipa_accepts_each_export_method() {
        for method in [
            "app-store-connect",
            "release-testing",
            "debugging",
            "enterprise",
        ] {
            let dir = temp_project(&format!("ipa-method-{method}"));
            fs::create_dir_all(dir.join("build/ios/ipa")).unwrap();
            fs::write(dir.join("build/ios/ipa/Runner.ipa"), b"fake").unwrap();
            let runner = base_runner()
                .with(
                    "security find-identity -v -p codesigning",
                    ok(r#"  1) H "Apple Development: Ada (TEAMID1234)""#),
                )
                .with(
                    "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=bGVhbg== DEVELOPMENT_TEAM=TEAMID1234 CODE_SIGN_STYLE=Automatic CODE_SIGNING_ALLOWED=YES CODE_SIGNING_REQUIRED=YES -allowProvisioningUpdates -allowProvisioningDeviceRegistration -archivePath build/ios/archive/Runner.xcarchive archive",
                    ok("Archive succeeded"),
                )
                .with(
                    "xcrun xcodebuild -exportArchive -archivePath build/ios/archive/Runner.xcarchive -exportOptionsPlist build/ios/ExportOptions.plist -exportPath build/ios/ipa -allowProvisioningUpdates",
                    ok("Export succeeded"),
                );
            let env = FakeEnv::new();
            build_with_env(
                &runner,
                &env,
                &dir,
                &info(BuildMode::Release),
                &IosArtifact::Ipa {
                    export_method: method.to_string(),
                },
                NO_EXTRA,
                &mut |_| {},
            )
            .unwrap();
            let plist = fs::read_to_string(dir.join("build/ios/ExportOptions.plist")).unwrap();
            assert!(
                plist.contains(&format!("<string>{method}</string>")),
                "{plist}"
            );
            let _ = fs::remove_dir_all(&dir);
        }
    }

    /// A `frust.toml` with an `[ios] identifier` and a declared `dev` flavor.
    const IDENTITY_TOML: &str = "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
         [ios]\nidentifier = \"com.acme.app\"\n\n\
         [flavors.dev]\napp-id-suffix = \".dev\"\napp-name-suffix = \" Dev\"\n";

    fn plutil(app: &Path, key: &str) -> String {
        format!(
            "plutil -extract {key} raw {}",
            app.join("Info.plist").display()
        )
    }

    /// Runs a no-codesign device `.app` build of `info` in a project with
    /// [`IDENTITY_TOML`], expecting exactly `argv_tail` after `-project
    /// ios/Runner.xcodeproj`, with the built bundle's plist answering `plist_id` and
    /// `plist_name`; returns the result and every `on_line` line.
    fn identity_build(
        tag: &str,
        info: &BuildInfo,
        configuration: &str,
        argv_tail: &str,
        plist_id: &str,
        plist_name: &str,
    ) -> (Result<BuiltArtifacts>, Vec<String>) {
        let dir = temp_project(tag);
        fs::write(dir.join("frust.toml"), IDENTITY_TOML).unwrap();
        plant_products(&dir, configuration, false, "Runner.app");
        let app = products_dir(&dir, configuration, false).join("Runner.app");
        let runner = base_runner()
            .with(
                format!("xcrun xcodebuild -project ios/Runner.xcodeproj {argv_tail}"),
                ok("Build succeeded"),
            )
            .with(plutil(&app, "CFBundleIdentifier"), ok(plist_id))
            .with(plutil(&app, "CFBundleDisplayName"), ok(plist_name));
        let mut lines = Vec::new();
        let result = build_with_env(
            &runner,
            &FakeEnv::new(),
            &dir,
            info,
            &IosArtifact::App {
                simulator: false,
                codesign: false,
            },
            NO_EXTRA,
            &mut |line| lines.push(line.to_string()),
        );
        let _ = fs::remove_dir_all(&dir);
        (result, lines)
    }

    const NO_CODESIGN: &str =
        "CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY= build";

    /// A flavor declared only in `frust.toml` (no `Dev` scheme in the
    /// project) builds `Runner` with the plain configuration and passes the
    /// flavor's identity; a bundle carrying it builds without a warning.
    #[test]
    fn a_declared_flavor_builds_runner_with_its_identity() {
        let mut dev = info(BuildMode::Release);
        dev.flavor = Some("dev".to_string());
        let (result, lines) = identity_build(
            "declared-flavor",
            &dev,
            "Release",
            &format!(
                "-scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=bGVhbg== PRODUCT_BUNDLE_IDENTIFIER=com.acme.app.dev FRUST_DISPLAY_NAME=Myapp Dev {NO_CODESIGN}"
            ),
            "com.acme.app.dev\n",
            "Myapp Dev\n",
        );
        assert_eq!(result.unwrap().paths.len(), 1);
        assert!(
            !lines.iter().any(|l| l.starts_with("warning:")),
            "{lines:?}"
        );
    }

    /// An explicit `[ios] identifier` with no flavor passes the identity on
    /// the plain `Runner` build.
    #[test]
    fn an_explicit_identifier_is_passed_without_a_flavor() {
        let (result, _) = identity_build(
            "explicit-id",
            &info(BuildMode::Release),
            "Release",
            &format!(
                "-scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=bGVhbg== PRODUCT_BUNDLE_IDENTIFIER=com.acme.app FRUST_DISPLAY_NAME=Myapp {NO_CODESIGN}"
            ),
            "com.acme.app\n",
            "Myapp\n",
        );
        assert_eq!(result.unwrap().paths.len(), 1);
    }

    #[test]
    fn a_built_bundle_identifier_mismatch_fails_naming_both() {
        let (result, _) = identity_build(
            "id-mismatch",
            &info(BuildMode::Release),
            "Release",
            &format!(
                "-scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=bGVhbg== PRODUCT_BUNDLE_IDENTIFIER=com.acme.app FRUST_DISPLAY_NAME=Myapp {NO_CODESIGN}"
            ),
            "dev.f0x.myapp\n",
            "Myapp\n",
        );
        let message = result.unwrap_err().to_string();
        assert_eq!(
            message,
            identifier_mismatch("dev.f0x.myapp", "com.acme.app")
        );
        assert!(
            message.contains("`dev.f0x.myapp`") && message.contains("`com.acme.app`"),
            "{message}"
        );
        assert!(
            message.contains("$(PRODUCT_BUNDLE_IDENTIFIER)"),
            "{message}"
        );
    }

    /// A project whose `Info.plist` predates `FRUST_DISPLAY_NAME` shows its
    /// literal name: one warning carrying both template lines.
    #[test]
    fn a_built_display_name_mismatch_warns_with_the_template_lines() {
        let mut dev = info(BuildMode::Release);
        dev.flavor = Some("dev".to_string());
        let (result, lines) = identity_build(
            "name-mismatch",
            &dev,
            "Release",
            &format!(
                "-scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=bGVhbg== PRODUCT_BUNDLE_IDENTIFIER=com.acme.app.dev FRUST_DISPLAY_NAME=Myapp Dev {NO_CODESIGN}"
            ),
            "com.acme.app.dev\n",
            "Myapp\n",
        );
        assert_eq!(result.unwrap().paths.len(), 1);
        let warnings: Vec<&String> = lines.iter().filter(|l| l.starts_with("warning:")).collect();
        assert_eq!(warnings, vec![&display_name_warning("Myapp", "Myapp Dev")]);
        let warning = warnings[0];
        assert!(
            warning.contains("<string>$(FRUST_DISPLAY_NAME)</string>"),
            "{warning}"
        );
        assert!(
            warning.contains("FRUST_DISPLAY_NAME = \"Myapp\";"),
            "{warning}"
        );
        assert_eq!(warning.lines().count(), 1, "{warning}");
    }

    /// A derived identity passes no identity settings and reads no plist:
    /// the build's only registered fixtures are the plain ones, so a
    /// `plutil` call or an extra setting would find none.
    #[test]
    fn a_derived_identity_passes_nothing_and_checks_nothing() {
        let dir = temp_project("derived-identity");
        plant_products(&dir, "Release", false, "Runner.app");
        let runner = base_runner().with(
            format!("xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios FRUST_FEATURES=bGVhbg== {NO_CODESIGN}"),
            ok("Build succeeded"),
        );
        let mut lines = Vec::new();
        check_built_identity(
            &runner,
            &products_dir(&dir, "Release", false).join("Runner.app"),
            resolve_identity(
                crate::manifest::load_optional(&dir).unwrap().as_ref(),
                &info(BuildMode::Release),
            )
            .unwrap()
            .as_ref(),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert!(lines.is_empty(), "{lines:?}");
        let artifacts = build_with_env(
            &runner,
            &FakeEnv::new(),
            &dir,
            &info(BuildMode::Release),
            &IosArtifact::App {
                simulator: false,
                codesign: false,
            },
            NO_EXTRA,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(artifacts.paths.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_undeclared_flavor_without_a_scheme_names_the_flavors_declaration() {
        let dir = temp_project("undeclared-flavor");
        fs::write(dir.join("frust.toml"), IDENTITY_TOML).unwrap();
        let mut qa = info(BuildMode::Release);
        qa.flavor = Some("qa".to_string());
        let err = build_with_env(
            &base_runner(),
            &FakeEnv::new(),
            &dir,
            &qa,
            &IosArtifact::App {
                simulator: false,
                codesign: false,
            },
            NO_EXTRA,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("`Qa`"), "{err}");
        assert!(err.to_string().contains("[flavors.qa]"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_ios_reports_actionable_error_when_scheme_missing() {
        let dir = temp_project("missing-scheme");
        let runner = base_runner();
        let env = FakeEnv::new();
        let mut info = info(BuildMode::Release);
        info.flavor = Some("paid".to_string());
        let err = build_with_env(
            &runner,
            &env,
            &dir,
            &info,
            &IosArtifact::App {
                simulator: false,
                codesign: false,
            },
            NO_EXTRA,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("Paid"), "{err}");
        assert!(err.to_string().contains("Available schemes"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_ios_surfaces_xcodebuild_failure_with_doctor_hint() {
        let dir = temp_project("build-fail");
        let runner = base_runner().with(
            format!("xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Release -sdk iphonesimulator -destination generic/platform=iOS Simulator -derivedDataPath build/ios ARCHS={} FRUST_FEATURES=bGVhbg== build", xcodebuild::host_sim_arch()),
            Output {
                success: false,
                stdout: "error: No signing certificate found".to_string(),
                stderr: String::new(),
            },
        );
        let env = FakeEnv::new();
        let err = build_with_env(
            &runner,
            &env,
            &dir,
            &info(BuildMode::Release),
            &IosArtifact::App {
                simulator: true,
                codesign: false,
            },
            NO_EXTRA,
            &mut |_| {},
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("No signing certificate found"),
            "{message}"
        );
        assert!(message.contains("frust doctor"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The decoded `FRUST_FEATURES` value for each key: the scaffolded
    /// `frust` key (and a project with no manifest at all) keep the canonical
    /// spelling; an app keyed `frust-ui` gets the mapped list, its
    /// passthrough after the mode's own; release keeps the app's `lean`.
    #[test]
    fn resolve_features_b64_spells_the_facade_key() {
        let decoded = |dir: &Path, mode: BuildMode, extra: &[String]| {
            let encoded = resolve_features_b64(dir, mode, extra, &mut |_| {})
                .unwrap()
                .unwrap();
            assert_eq!(
                encoded,
                base64_encode(decode_csv_for_test(&encoded).as_bytes()),
                "round trip"
            );
            decode_csv_for_test(&encoded)
        };

        let bare = temp_project("features-no-manifest");
        assert_eq!(
            decoded(&bare, BuildMode::Debug, NO_EXTRA),
            "frust/perf-trace,frust/devtools"
        );

        let scaffolded = temp_project("features-scaffolded");
        fs::write(
            scaffolded.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
             frust = { package = \"frust-ui\", version = \"0.6.0\" }\n",
        )
        .unwrap();
        assert_eq!(
            decoded(&scaffolded, BuildMode::Debug, NO_EXTRA),
            "frust/perf-trace,frust/devtools"
        );

        let keyed = temp_project("features-frust-ui");
        fs::write(
            keyed.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
             frust-ui = \"0.6.0\"\n\n[features]\nlean = []\n",
        )
        .unwrap();
        assert_eq!(
            decoded(&keyed, BuildMode::Debug, &["hybrid-tier".to_string()]),
            "frust-ui/perf-trace,frust-ui/devtools,hybrid-tier"
        );
        assert_eq!(
            decoded(&keyed, BuildMode::Profile, NO_EXTRA),
            "frust-ui/perf-trace,frust-ui/devtools"
        );
        assert_eq!(decoded(&keyed, BuildMode::Release, NO_EXTRA), "lean");

        for dir in [bare, scaffolded, keyed] {
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn an_ambiguous_facade_key_fails_feature_resolution() {
        let dir = temp_project("features-ambiguous");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
             frust = { package = \"frust-ui\", version = \"0.6.0\" }\n\
             ui = { package = \"frust-ui\", version = \"0.6.0\" }\n",
        )
        .unwrap();
        let err = resolve_features_b64(&dir, BuildMode::Debug, NO_EXTRA, &mut |_| {}).unwrap_err();
        assert!(format!("{err:#}").contains("more than one key"), "{err:#}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_feature_failure_carries_the_facade_key_hint() {
        let out = Output {
            success: false,
            stdout: "error: the package 'app' does not contain this feature: frust/devtools"
                .to_string(),
            stderr: String::new(),
        };
        let message = failure_message(&out, BuildMode::Debug);
        assert!(message.contains(FacadeKey::hint()), "{message}");
        assert!(message.ends_with("Xcode/signing setup."), "{message}");
        assert!(!failure_message(&out, BuildMode::Release).contains(FacadeKey::hint()));
    }

    /// Standard base64 decode of a `FRUST_FEATURES` value, test-only (the
    /// drive never decodes; the template's run-script does).
    fn decode_csv_for_test(encoded: &str) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bits: u32 = 0;
        let mut count = 0;
        let mut out = Vec::new();
        for byte in encoded.bytes().filter(|b| *b != b'=') {
            let value = TABLE.iter().position(|t| *t == byte).unwrap() as u32;
            bits = (bits << 6) | value;
            count += 6;
            if count >= 8 {
                count -= 8;
                out.push((bits >> count) as u8);
                bits &= (1 << count) - 1;
            }
        }
        String::from_utf8(out).unwrap()
    }
}
