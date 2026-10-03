use std::path::Path;

use anyhow::{Context, Result};
use frust_drive::doctor::report::ComponentStatus;
use frust_drive::doctor::{DoctorCtx, RealEnv, Status, Validation, Validator};
use frust_drive::packages::{CachedLocator, CargoLocator, PackageLocator};
use frust_drive::platform_wiring;
use frust_drive::process::ProcessRunner;
use frust_drive::web_build::{self, WebPreflight};

/// Runs all doctor validators and prints their results, then the browser
/// preflight under its own "Web" heading, and returns the process exit code
/// by the one rule [`exit_code`] states: `1` if any validator is `Fail`, else
/// `0`. The browser checks ([`web_build::preflight`]) are informational only
/// and never affect the exit code — a host without wasm-bindgen or a
/// directory without a frust path dependency is not a doctor failure.
///
/// This entry point is where every host-bound input is fixed: the process
/// runner injected by `commands::dispatch` (the CLI's one `Real` construction
/// site), the [`RealEnv`] lookup seam, the host's own `target_os`, the
/// current directory, and [`frust_drive::doctor::default_validators_for_host`].
/// All of them are parameters of [`run_with`], which is why the exit-code
/// rule can be tested against a scripted environment rather than against
/// whatever toolchains the machine running the test happens to have.
///
/// The host's browser toolchain — the wasm32 target, the `wasm-bindgen` CLI
/// and `wasm-opt` — is reported by the validator list above, by the three
/// validators `frust_drive::doctor::default_validators_for_host` registers
/// for it (each `Partial` at worst, so none of them can move the exit code).
/// The same call also decides whether an `Xcode` row is registered at all:
/// only on a macOS host, matching `report::build_report`'s iOS-area gate. The
/// "Web" heading below therefore prints only what those rows cannot know:
/// the *project's* own browser inputs — its `frust.toml [web]` section, the
/// host page that would be staged, that page's module name, the artifact
/// directory's safety, and a `wasm-bindgen` pin the project declares that the
/// installed CLI disagrees with. That subset is
/// [`frust_drive::web_build::WebPreflight::project_rows`]'s, chosen there
/// rather than here so the filter lives beside the rows it names.
///
/// Each Web row prints the preflight's own icon and summary exactly as the
/// browser pipeline reports it: the rows are excluded from the exit code, not
/// reworded.
///
/// Inside a Frust project (a directory holding `frust.toml`) a last
/// "Platform packages" heading prints where the project's Android, iOS and
/// web embeddings resolve — the directories `frust run`/`frust build` wire
/// the host projects to — or why they could not be located. Informational
/// too: it never moves the exit code.
pub fn run_in(runner: &dyn ProcessRunner, verbose: bool) -> Result<u8> {
    let env = RealEnv;
    let ctx = DoctorCtx {
        runner,
        env: &env,
        is_macos: cfg!(target_os = "macos"),
    };
    // The browser preflight is project-aware (it reports the resolved host
    // page and `[web]` manifest section), so it runs against the current
    // directory the same way `frust build`/`frust run` resolve their own
    // project root — a directory that is no project still reports its rows
    // honestly, degraded rather than failed (see `web_build::preflight`'s own
    // doc comment).
    let cwd = std::env::current_dir().context("reading current directory")?;
    let validators = frust_drive::doctor::default_validators_for_host(ctx.is_macos);
    Ok(run_with(&ctx, &validators, &cwd, verbose))
}

/// The testable core of [`run_in`]: the validator set, the context they run
/// under (process runner, env lookup, host OS) and the project directory the
/// browser preflight inspects are all parameters, so nothing about the
/// machine running a test leaks into the exit code it asserts.
fn run_with(
    ctx: &DoctorCtx<'_>,
    validators: &[Box<dyn Validator>],
    project_dir: &Path,
    verbose: bool,
) -> u8 {
    let results = frust_drive::doctor::run_all(ctx, validators);
    print_results(&results, verbose);

    let web_preflight = web_build::preflight(ctx.runner, project_dir);
    print_web_results(&web_preflight, verbose);

    if project_dir.join("frust.toml").is_file() {
        // One cached locator serves all three rows: a single `cargo metadata`
        // run, through the injected runner.
        let cargo = CargoLocator::new(ctx.runner);
        let locator = CachedLocator::new(&cargo);
        for line in package_dir_lines(&locator, project_dir) {
            println!("{line}");
        }
    }

    exit_code(&results)
}

/// The "Platform packages" heading's lines: one row per platform naming the
/// embedding directory the project resolves, or the error that prevented
/// it. Android and iOS come from one [`platform_wiring::resolve`] through
/// `locator`; web is the browser host page [`web_build::embedder_dir_with`]
/// resolves through the same locator.
fn package_dir_lines(locator: &dyn PackageLocator, project_dir: &Path) -> Vec<String> {
    let (android, ios) = match platform_wiring::resolve(locator, project_dir) {
        Ok(dirs) => (Ok(dirs.android), Ok(dirs.ios)),
        Err(err) => {
            let message = err.to_string();
            (Err(message.clone()), Err(message))
        }
    };
    let web = web_build::embedder_dir_with(locator, project_dir).map_err(|err| err.to_string());

    let mut lines = vec!["Platform packages".to_string()];
    for (name, resolved) in [("android", android), ("ios", ios), ("web", web)] {
        match resolved {
            Ok(dir) => {
                lines.push(format!("[\u{2713}] {name}"));
                lines.push(format!("    {}", dir.display()));
            }
            Err(message) => {
                lines.push(format!("[\u{2717}] {name}"));
                lines.push(format!("    {message}"));
            }
        }
    }
    lines
}

/// The exit-code rule in one place: `1` if any validator is `Fail`, else
/// `0`. Deliberately takes only the validator results — the browser
/// preflight is printed by [`run_with`] but never consulted here, which is
/// what makes its rows informational.
fn exit_code(results: &[(String, Validation)]) -> u8 {
    let any_fail = results
        .iter()
        .any(|(_, validation)| validation.status == Status::Fail);
    if any_fail { 1 } else { 0 }
}

fn print_results(results: &[(String, Validation)], verbose: bool) {
    for (name, validation) in results {
        let icon = match validation.status {
            Status::Pass => "[\u{2713}]",
            Status::Partial => "[!]",
            Status::Fail => "[\u{2717}]",
        };
        println!("{icon} {name}");
        if verbose || validation.status != Status::Pass {
            for message in &validation.messages {
                println!("    {message}");
            }
        }
    }
}

/// Renders [`WebPreflight`]'s project-dependent rows under a "Web" heading,
/// in the same icon/indented-message shape [`print_results`] uses for the flat
/// validator list above — a browser-build row is exactly as actionable as a
/// validator one, just carried in `frust-drive`'s newer `Component` shape
/// rather than the older `Validation` one.
///
/// Only [`WebPreflight::project_rows`] is printed: the host-tool rows are the
/// three web validators' job in the list above, and printing them twice would
/// state the same gap in two different severities (see [`run_in`]'s doc
/// comment).
fn print_web_results(preflight: &WebPreflight, verbose: bool) {
    println!("Web");
    for component in preflight.project_rows() {
        let icon = match component.status {
            ComponentStatus::Ok => "[\u{2713}]",
            ComponentStatus::Partial => "[!]",
            ComponentStatus::Missing => "[\u{2717}]",
        };
        println!("{icon} {}", component.name);
        if verbose || component.status != ComponentStatus::Ok {
            println!("    {}", component.summary);
            for fix in &component.fix_commands {
                println!("    fix: {}", fix.display);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::doctor::EnvLookup;
    use frust_drive::process::{FakeProcessRunner, Output};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// An env lookup that knows no variable at all — no `ANDROID_HOME`, no
    /// `CARGO_TARGET_DIR` — so nothing this machine exports reaches a
    /// validator under test.
    struct EmptyEnv;

    impl EnvLookup for EmptyEnv {
        fn get(&self, _key: &str) -> Option<String> {
            None
        }
    }

    /// A validator with a fixed answer, standing in for the real set so the
    /// exit-code rule is established against a known input rather than
    /// against whichever toolchains the test host has installed.
    struct Fixed(Status);

    impl Validator for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }

        fn validate(&self, _ctx: &DoctorCtx) -> Validation {
            Validation {
                status: self.0,
                messages: vec!["scripted".to_string()],
            }
        }
    }

    fn fixed(statuses: &[Status]) -> Vec<Box<dyn Validator>> {
        statuses
            .iter()
            .map(|status| Box::new(Fixed(*status)) as Box<dyn Validator>)
            .collect()
    }

    fn ctx<'a>(runner: &'a FakeProcessRunner, env: &'a EmptyEnv) -> DoctorCtx<'a> {
        DoctorCtx {
            runner,
            env,
            is_macos: false,
        }
    }

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    /// A fresh directory that is not a project (no `Cargo.toml`), so the
    /// browser preflight's project rows degrade the way `frust doctor` in an
    /// arbitrary directory sees them.
    fn empty_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("frust-cli-doctor-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn row(status: Status) -> (String, Validation) {
        (
            "row".to_string(),
            Validation {
                status,
                messages: Vec::new(),
            },
        )
    }

    #[test]
    fn exit_code_is_one_iff_a_validator_fails() {
        assert_eq!(exit_code(&[]), 0);
        assert_eq!(exit_code(&[row(Status::Pass), row(Status::Partial)]), 0);
        assert_eq!(exit_code(&[row(Status::Pass), row(Status::Fail)]), 1);
    }

    /// The contract in one scripted run: a browser preflight that is not
    /// ready (an empty runner knows no `rustup` and no `wasm-bindgen`, and
    /// the directory is no project) leaves the exit code at 0 when every
    /// validator passes — the Web rows are printed, never counted.
    #[test]
    fn a_not_ready_web_preflight_never_changes_the_exit_code() {
        let dir = empty_dir("not-ready");
        let runner = FakeProcessRunner::new();
        let env = EmptyEnv;
        assert!(
            !web_build::preflight(&runner, &dir).is_ready(),
            "precondition: the preflight under test must be degraded"
        );
        let validators = fixed(&[Status::Pass, Status::Partial]);
        assert_eq!(run_with(&ctx(&runner, &env), &validators, &dir, false), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other half of the same rule: a failing validator exits 1 whether
    /// or not the Web rows are healthy — the preflight cannot rescue a run
    /// any more than it can sink one.
    #[test]
    fn a_failing_validator_exits_one_regardless_of_the_web_rows() {
        let dir = empty_dir("validator-fail");
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("wasm32-unknown-unknown\n"),
            )
            .with("wasm-bindgen --version", ok("wasm-bindgen 0.2.128\n"))
            .with("wasm-opt --version", ok("wasm-opt version 130\n"));
        let env = EmptyEnv;
        let validators = fixed(&[Status::Pass, Status::Fail]);
        assert_eq!(run_with(&ctx(&runner, &env), &validators, &dir, false), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The delta form of the rule, on the row that first motivated it:
    /// the same validator set yields the same exit code with `wasm-bindgen`
    /// present and with it absent.
    #[test]
    fn the_wasm_bindgen_row_is_informational() {
        let dir = empty_dir("bindgen-delta");
        let env = EmptyEnv;
        let validators = fixed(&[Status::Pass]);
        let present =
            FakeProcessRunner::new().with("wasm-bindgen --version", ok("wasm-bindgen 0.2.128\n"));
        let absent = FakeProcessRunner::new().missing("wasm-bindgen --version");
        assert_eq!(
            run_with(&ctx(&present, &env), &validators, &dir, false),
            run_with(&ctx(&absent, &env), &validators, &dir, false),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An env lookup with a scripted map, for the one test that runs the
    /// *real* validator set and therefore needs the Android variables set.
    struct MapEnv(&'static [(&'static str, &'static str)]);

    impl EnvLookup for MapEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    /// The rule the three web validators were added under, on the real
    /// validator set: a host with no browser toolchain at all still exits 0.
    ///
    /// Runs through [`run_with`] rather than [`run_in`] because `run_in`
    /// binds the real environment and the real current directory, which would
    /// make the assertion depend on the machine running the test — the same
    /// reason every other exit-code test here is scripted.
    #[test]
    fn a_wholly_absent_web_toolchain_exits_zero_on_the_real_validator_set() {
        let dir = empty_dir("web-partial");
        let runner = FakeProcessRunner::new()
            .with(
                "rustc --version",
                ok("rustc 1.91.1 (ed61e7d7e 2025-11-07)\n"),
            )
            .with("cargo --version", ok("cargo 1.91.1\n"))
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\narmv7-linux-androideabi\nx86_64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .with("adb version", ok("Android Debug Bridge version 1.0.41\n"))
            .with("cargo packager --version", ok("cargo-packager 0.11.8\n"))
            .missing("wasm-bindgen --version")
            .missing("wasm-opt --version");
        let env = MapEnv(&[
            ("ANDROID_HOME", "/sdk"),
            ("ANDROID_NDK_HOME", "/sdk/ndk/26.1.10909125"),
        ]);
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: false,
        };
        let validators = frust_drive::doctor::default_validators_for_host(ctx.is_macos);
        let results = frust_drive::doctor::run_all(&ctx, &validators);
        for name in ["wasm32 target", "wasm-bindgen CLI", "wasm-opt"] {
            let row = results
                .iter()
                .filter(|(n, _)| n == name)
                .collect::<Vec<_>>();
            assert_eq!(row.len(), 1, "`{name}` must appear exactly once");
            assert_eq!(row[0].1.status, Status::Partial, "{name}");
        }
        assert!(
            !results.iter().any(|(n, _)| n == "Xcode"),
            "a non-macOS host must have no Xcode row: {results:?}"
        );
        assert_eq!(run_with(&ctx, &validators, &dir, false), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The macOS counterpart of the test above, on the same real validator
    /// set: a host reported as macOS carries exactly one `Xcode` row.
    #[test]
    fn a_macos_host_gets_exactly_one_xcode_row_in_the_real_validator_set() {
        let dir = empty_dir("web-macos");
        let runner = FakeProcessRunner::new()
            .with(
                "rustc --version",
                ok("rustc 1.91.1 (ed61e7d7e 2025-11-07)\n"),
            )
            .with("cargo --version", ok("cargo 1.91.1\n"))
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\narmv7-linux-androideabi\nx86_64-linux-android\n"),
            )
            .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
            .with("adb version", ok("Android Debug Bridge version 1.0.41\n"))
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "xcodebuild -version",
                ok("Xcode 15.4\nBuild version 15F31d\n"),
            )
            .with("cargo packager --version", ok("cargo-packager 0.11.8\n"))
            .missing("wasm-bindgen --version")
            .missing("wasm-opt --version");
        let env = MapEnv(&[
            ("ANDROID_HOME", "/sdk"),
            ("ANDROID_NDK_HOME", "/sdk/ndk/26.1.10909125"),
        ]);
        let ctx = DoctorCtx {
            runner: &runner,
            env: &env,
            is_macos: true,
        };
        let validators = frust_drive::doctor::default_validators_for_host(ctx.is_macos);
        let results = frust_drive::doctor::run_all(&ctx, &validators);
        let xcode_rows: Vec<_> = results.iter().filter(|(n, _)| n == "Xcode").collect();
        assert_eq!(
            xcode_rows.len(),
            1,
            "a macOS host must have exactly one Xcode row: {results:?}"
        );
        assert_eq!(xcode_rows[0].1.status, Status::Pass);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Web heading is now the project's rows only — the host-tool rows it
    /// used to duplicate belong to the validator list above.
    #[test]
    fn the_web_heading_drops_the_rows_the_validators_now_own() {
        let dir = empty_dir("web-heading");
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("wasm32-unknown-unknown\n"),
            )
            .with("wasm-bindgen --version", ok("wasm-bindgen 0.2.128\n"))
            .with("wasm-opt --version", ok("wasm-opt version 130\n"));
        let preflight = web_build::preflight(&runner, &dir);
        let printed: Vec<&str> = preflight
            .project_rows()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        for host_row in [
            web_build::TARGET_COMPONENT,
            web_build::BINDGEN_COMPONENT,
            web_build::WASM_OPT_COMPONENT,
        ] {
            assert!(!printed.contains(&host_row), "{host_row} still printed");
        }
        assert!(printed.contains(&web_build::EMBEDDER_COMPONENT));
        print_web_results(&preflight, true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The production entry point runs end to end on any host: the only error
    /// it can return is an unreadable current directory. The exit code is
    /// deliberately not asserted here — it depends on this machine's
    /// toolchains, which is exactly what the scripted tests above avoid.
    #[test]
    fn run_in_reaches_the_web_preflight() {
        run_in(&FakeProcessRunner::new(), false).expect("doctor runs to completion");
    }

    /// The "Platform packages" rows name each resolved directory, and a
    /// locate failure is printed in place of the rows it prevented.
    #[test]
    fn package_dir_lines_name_each_directory_or_the_error() {
        use frust_drive::packages::StubLocator;

        let root = empty_dir("package-dirs");
        let android = root.join("frust-shell-android");
        let ios = root.join("frust-shell-ios");
        std::fs::create_dir_all(android.join(platform_wiring::ANDROID_EMBEDDING_REL)).unwrap();
        std::fs::create_dir_all(ios.join(platform_wiring::IOS_EMBEDDING_REL)).unwrap();
        let root = root.canonicalize().unwrap();
        let stub = StubLocator::new()
            .with(
                platform_wiring::ANDROID_SHELL_PACKAGE,
                root.join("frust-shell-android"),
            )
            .with(
                platform_wiring::IOS_SHELL_PACKAGE,
                root.join("frust-shell-ios"),
            );
        let web_dir = root.join("frust-shell-web/platform/web");
        std::fs::create_dir_all(&web_dir).unwrap();
        for file in ["index.html", "frust_web.js"] {
            std::fs::write(web_dir.join(file), "x").unwrap();
        }
        let stub = stub.with("frust-shell-web", root.join("frust-shell-web"));

        let lines = package_dir_lines(&stub, &root);
        assert_eq!(
            lines,
            vec![
                "Platform packages".to_string(),
                "[\u{2713}] android".to_string(),
                format!(
                    "    {}",
                    root.join("frust-shell-android")
                        .join(platform_wiring::ANDROID_EMBEDDING_REL)
                        .display()
                ),
                "[\u{2713}] ios".to_string(),
                format!(
                    "    {}",
                    root.join("frust-shell-ios")
                        .join(platform_wiring::IOS_EMBEDDING_REL)
                        .display()
                ),
                "[\u{2713}] web".to_string(),
                format!("    {}", web_dir.display()),
            ]
        );

        let offline = StubLocator::failing("network is unreachable");
        let lines = package_dir_lines(&offline, &root);
        assert_eq!(lines.len(), 7, "{lines:?}");
        for (row, name) in [(1, "android"), (3, "ios"), (5, "web")] {
            assert_eq!(lines[row], format!("[\u{2717}] {name}"), "{lines:?}");
        }
        assert!(lines[2].contains("network is unreachable"), "{lines:?}");
        assert!(lines[4].contains("network is unreachable"), "{lines:?}");
        assert!(lines[6].contains("network is unreachable"), "{lines:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// [`print_web_results`] never panics on an empty component list (a
    /// defensive shape check for the rendering helper itself, independent of
    /// whatever `web_build::preflight` reports on this host).
    #[test]
    fn print_web_results_handles_empty_components() {
        print_web_results(
            &WebPreflight {
                components: vec![],
                bindgen_row_kind: web_build::BindgenRowKind::HostOnly,
            },
            true,
        );
    }
}
