//! iOS release build pipeline (spec §12.6): `frust build ios` (device or
//! simulator `.app`) and `frust build ipa` (archive + `-exportArchive`).
//!
//! The [`build`] signature and the [`IosArtifact`]/[`BuiltArtifacts`] types
//! are load-bearing for `commands::build` and stay frozen; this module owns
//! the `xcodebuild`-driving body — scheme/configuration resolution
//! ([`schemes`]), `DEVELOPMENT_TEAM` auto-detect ([`team`]), argv assembly
//! ([`xcodebuild`]), and exportOptions plist generation ([`export`]).

mod export;
mod schemes;
// `pub(crate)`: `doctor::report`'s Signing component (Plan D6a) reuses
// `team::detect` rather than re-implementing `security find-identity`
// parsing.
pub(crate) mod team;
pub(crate) mod xcodebuild;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::build_info::BuildInfo;
use crate::doctor::{EnvLookup, RealEnv};
use crate::process::{Output, ProcessRunner, tail_lines};

use self::xcodebuild::{Invocation, Signing};

/// The artifact `frust build ios`/`ipa` requests (spec §12.6).
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
/// **Frozen signature** — do not change it without updating every caller
/// (`commands::build`, `frust-tui`'s build session, `ios_run`'s physical-run
/// session) and this doc comment. The `on_line` sink was added by the
/// tty-garbling fix.
pub fn build(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    target: &IosArtifact,
    on_line: &mut dyn FnMut(&str),
) -> Result<BuiltArtifacts> {
    build_with_env(runner, &RealEnv, project_dir, info, target, on_line)
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
    on_line: &mut dyn FnMut(&str),
) -> Result<BuiltArtifacts> {
    let sc = schemes::resolve(info);
    let simulator = matches!(
        target,
        IosArtifact::App {
            simulator: true,
            ..
        }
    );

    preflight(runner, simulator).map_err(|err| anyhow::anyhow!(err))?;
    schemes::verify(runner, project_dir, &sc)?;

    let defines_b64 = encode_defines(&info.defines);
    // Release-lean preflight (followup F2): a legacy app that predates the
    // `lean` feature has it dropped here (with a one-time warning through the
    // `on_line` sink) so xcodebuild never threads an undeclared `--features
    // lean` down to `cargo` — cargo's opaque hard error. A declaring app keeps
    // byte-identical features and warns nothing.
    let (features, warning) =
        crate::cargo_manifest::resolve_release_features(project_dir, info.mode);
    if let Some(warning) = warning {
        on_line(&warning);
    }
    let features_b64 = encode_features(&features);

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
                marketing_version: info.build_name.as_deref(),
                current_project_version: info.build_number,
                defines_b64: defines_b64.as_deref(),
                features_b64: features_b64.as_deref(),
            };
            run_xcodebuild(runner, project_dir, &inv.build_argv(), on_line)?;

            let products = products_dir(project_dir, &sc.configuration, *simulator);
            let app = glob_one(&products, "app").with_context(|| {
                format!("locating the built `.app` under `{}`", products.display())
            })?;
            Ok(BuiltArtifacts { paths: vec![app] })
        }
        IosArtifact::Ipa { export_method } => {
            // `Ipa` carries no codesign flag by construction — signing is
            // always required for an archive/export.
            let choice = team::resolve(env, runner, project_dir)?;
            if let Some(note) = &choice.note {
                on_line(note);
            }

            let inv = Invocation {
                scheme: &sc.scheme,
                configuration: &sc.configuration,
                simulator: false,
                signing: Signing::Automatic {
                    team: choice.team.clone(),
                },
                marketing_version: info.build_name.as_deref(),
                current_project_version: info.build_number,
                defines_b64: defines_b64.as_deref(),
                features_b64: features_b64.as_deref(),
            };
            run_xcodebuild(runner, project_dir, &inv.archive_argv(), on_line)?;

            let plist = export::export_options_plist(export_method, Some(&choice.team));
            let plist_path = project_dir.join(export::EXPORT_OPTIONS_PATH);
            if let Some(parent) = plist_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating `{}`", parent.display()))?;
            }
            fs::write(&plist_path, plist)
                .with_context(|| format!("writing `{}`", plist_path.display()))?;

            run_xcodebuild(runner, project_dir, &export::export_argv(), on_line)?;

            let ipa_dir = project_dir.join(export::EXPORT_PATH);
            let ipa = glob_one(&ipa_dir, "ipa").with_context(|| {
                format!("locating the exported `.ipa` under `{}`", ipa_dir.display())
            })?;
            Ok(BuiltArtifacts { paths: vec![ipa] })
        }
    }
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

/// Streams an `xcrun xcodebuild …` invocation, prefixing each line, and turns
/// a non-zero exit into an error whose message tails both stdout and stderr
/// (xcodebuild reports most errors on stdout) plus a `frust doctor` hint —
/// signing errors pass through verbatim (spec §16).
fn run_xcodebuild(
    runner: &dyn ProcessRunner,
    root: &Path,
    argv: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let mut prefixed = |line: &str| on_line(&format!("[xcodebuild] {line}"));
    let out = runner
        .run_streaming("xcrun", &refs, Some(root), &[], &mut prefixed)
        .with_context(|| format!("running `xcodebuild` in `{}`", root.display()))?;
    if !out.success {
        bail!("{}", failure_message(&out));
    }
    Ok(())
}

fn failure_message(out: &Output) -> String {
    let stdout_tail = tail_lines(&out.stdout, 50);
    let stderr_tail = tail_lines(&out.stderr, 50);
    let mut message = "`xcodebuild` failed".to_string();
    if !stdout_tail.is_empty() {
        message.push_str(&format!("\n--- stdout (tail) ---\n{stdout_tail}"));
    }
    if !stderr_tail.is_empty() {
        message.push_str(&format!("\n--- stderr (tail) ---\n{stderr_tail}"));
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
/// `base64 -d` decode into `--features <csv>` (release-lean plan, task 04).
/// `None` when there are no features to pass — the release-lean preflight can
/// legitimately produce an empty list for a legacy app whose `lean` feature
/// was dropped (`cargo_manifest::resolve_release_features`), in which case no
/// `FRUST_FEATURES=` setting is emitted at all. Shared with the simulator run
/// path (`ios_run::xcodebuild`) so the feature encoding has one source.
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
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(artifacts.paths.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Followup F2, legacy direction: a `--release` build against an app whose
    /// Cargo.toml declares no `lean` feature drops it and warns once through
    /// `on_line`; the xcodebuild invocation carries NO `FRUST_FEATURES=` build
    /// setting (the fixture omits it), so a regression that kept `lean` would
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

    /// Followup F2, declaring direction: an app that declares `lean` keeps it —
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
}
