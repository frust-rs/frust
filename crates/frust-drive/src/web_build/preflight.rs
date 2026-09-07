//! The environment check a browser build runs before it starts, in the
//! `doctor` module's own vocabulary.
//!
//! Three host tools decide whether [`super::build`] can succeed, and all three
//! fail late and opaquely without a check: a missing `wasm32-unknown-unknown`
//! target surfaces as a cargo error most Rust developers have never seen, a
//! `wasm-bindgen` CLI whose version differs from the crate's fails *after* a
//! full release compile with a schema-mismatch message, and a missing
//! `wasm-opt` silently ships an artifact several megabytes larger than the one
//! the project's own size numbers were measured against. Two more rows check
//! what [`super::build`] itself checks before compiling: which host page it
//! would stage ([`EMBEDDER_COMPONENT`], now aware of an app's own `[web]
//! host-dir`, not just the framework fallback) and, when that page is the
//! app's own, whether its `?module=` default agrees with the `--out-name`
//! this build would resolve ([`HOST_PAGE_COMPONENT`]).
//!
//! # Reported in `doctor`'s shape, not a shape of its own
//!
//! Every check comes back as a [`Component`] — the same struct
//! [`crate::doctor::report`] hands the TUI's bootstrap wizard and toolchain
//! chip, carrying a [`ComponentStatus`] and zero or more structured
//! [`FixCommand`]s. Reusing it means a front-end adding a browser row to
//! `frust doctor` renders it with the code it already has, and a fix command
//! it can offer to run is a fix command here too, rather than a string it has
//! to parse back out of a message.
//!
//! # The version rule is exact, and it is not semver
//!
//! `wasm-bindgen`'s generated glue and its CLI share a schema version, and the
//! crate's own tooling refuses a mismatch outright: the pair is a lockstep
//! contract, not a compatible-range one. That is why this repository pins the
//! crate exactly (`=0.2.128`, derived from the host CLI — see
//! `examples/web-spike/Cargo.toml`'s own derivation) and why
//! [`bindgen_check`] compares for *equality* against whatever the project
//! declares rather than asking whether one satisfies the other. A project
//! declaring no `wasm-bindgen` dependency at all is reported as
//! [`ComponentStatus::Partial`], never `Ok`: the CLI is installed, but there
//! is nothing to check it against, and saying so is more useful than a green
//! row that proved nothing.
//!
//! # What is fatal and what is only worse
//!
//! [`WebPreflight::is_ready`] is false only for a [`ComponentStatus::Missing`]
//! row. `wasm-opt` can never produce one — it is a size optimization, and a
//! build without it runs identically — so its absence is a `Partial` a front
//! end may report and proceed past. The target, the CLI, the manifest, the
//! embedder and the host-page-module check can: without any one of them there
//! is no build [`super::build`] would actually run to completion.

use std::path::Path;

use crate::build_info::WASM_TARGET_TRIPLE;
use crate::doctor::report::{Component, ComponentStatus, FixCommand};
use crate::manifest::{self, WebSection};
use crate::process::ProcessRunner;

use super::bundle;

/// The component names this module reports under. Named constants because a
/// front-end filtering or ordering the rows should key off the same strings
/// this module emits.
pub const MANIFEST_COMPONENT: &str = "frust.toml [web] section";
pub const TARGET_COMPONENT: &str = "wasm32 Rust target";
pub const BINDGEN_COMPONENT: &str = "wasm-bindgen CLI";
pub const WASM_OPT_COMPONENT: &str = "wasm-opt";
pub const EMBEDDER_COMPONENT: &str = "Browser host page";
pub const HOST_PAGE_COMPONENT: &str = "Host page module name";

/// Every browser-build environment check, in report order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebPreflight {
    pub components: Vec<Component>,
}

impl WebPreflight {
    /// The worst status across the checks — the single value a summary line or
    /// a status chip renders.
    pub fn status(&self) -> ComponentStatus {
        if self
            .components
            .iter()
            .any(|c| c.status == ComponentStatus::Missing)
        {
            ComponentStatus::Missing
        } else if self
            .components
            .iter()
            .any(|c| c.status == ComponentStatus::Partial)
        {
            ComponentStatus::Partial
        } else {
            ComponentStatus::Ok
        }
    }

    /// Whether a build can be attempted at all: true unless something the
    /// pipeline cannot run without is absent. A `Partial` row (an unverifiable
    /// version, a missing `wasm-opt`) degrades the result, never blocks it.
    pub fn is_ready(&self) -> bool {
        self.status() != ComponentStatus::Missing
    }

    /// The rows that make [`is_ready`](Self::is_ready) false — what a refusal
    /// message should list, rather than the whole report.
    pub fn blocking(&self) -> Vec<&Component> {
        self.components
            .iter()
            .filter(|c| c.status == ComponentStatus::Missing)
            .collect()
    }
}

/// Checks everything [`super::build`] needs from the host and the project.
///
/// Loads the project's `frust.toml` itself (`crate::manifest::load_optional`,
/// the same optional-input contract [`super::build`] uses) so the embedder and
/// host-page-module rows reflect the project's actual `[web]` section rather
/// than always assuming the defaults.
///
/// Never fails: an unrunnable probe is a `Missing` row with a fix command, not
/// an error — the entire point is to answer "what is wrong" in one pass
/// instead of surfacing the first problem and hiding the rest.
pub fn preflight(runner: &dyn ProcessRunner, project_dir: &Path) -> WebPreflight {
    let (manifest_component, manifest) = manifest_check(project_dir);
    let crate_name = bundle::package_name(project_dir).ok();
    let app_name = manifest
        .as_ref()
        .map(|m| m.app.name.clone())
        .or(crate_name)
        .unwrap_or_default();
    let web = manifest
        .as_ref()
        .and_then(|m| m.web.clone())
        .unwrap_or_default();

    WebPreflight {
        components: vec![
            manifest_component,
            target_check(runner),
            bindgen_check(runner, project_dir),
            wasm_opt_check(runner),
            embedder_check(project_dir, &web),
            host_page_module_check(project_dir, &web, &app_name),
        ],
    }
}

/// The project's `frust.toml`, read the same optional way [`super::build`]
/// does: `Ok(None)` for a genuinely absent manifest reports `Ok` (this
/// pipeline needs none), and a present-but-unparsable one reports `Missing`
/// with the parse error — the same reason [`super::build`] itself refuses to
/// build against one.
fn manifest_check(project_dir: &Path) -> (Component, Option<crate::manifest::Manifest>) {
    let name = MANIFEST_COMPONENT.to_string();
    match manifest::load_optional(project_dir) {
        Ok(Some(manifest)) => (
            Component {
                name,
                status: ComponentStatus::Ok,
                summary: "frust.toml read".to_string(),
                fix_commands: Vec::new(),
            },
            Some(manifest),
        ),
        Ok(None) => (
            Component {
                name,
                status: ComponentStatus::Ok,
                summary: "no frust.toml — using [web] section defaults".to_string(),
                fix_commands: Vec::new(),
            },
            None,
        ),
        Err(err) => (
            Component {
                name,
                status: ComponentStatus::Missing,
                summary: format!("{err:#}"),
                fix_commands: Vec::new(),
            },
            None,
        ),
    }
}

/// `wasm32-unknown-unknown` installed on the active toolchain.
///
/// Probes `rustup` directly rather than through
/// `doctor::mobile_targets`'s shared installed-target probe: that module is
/// private to `doctor`, and reaching it would mean widening a surface for one
/// three-line call. The invocation is identical, so the two never disagree
/// about what "installed" means.
fn target_check(runner: &dyn ProcessRunner) -> Component {
    let installed = match runner.run("rustup", &["target", "list", "--installed"]) {
        Ok(out) if out.success => out.stdout,
        _ => {
            return Component {
                name: TARGET_COMPONENT.to_string(),
                status: ComponentStatus::Missing,
                summary: "rustup could not be run, so the installed targets are unknown"
                    .to_string(),
                fix_commands: vec![FixCommand {
                    display: "Install rustup".to_string(),
                    program: String::new(),
                    args: Vec::new(),
                    auto_runnable: false,
                    doc_link: Some("https://rustup.rs".to_string()),
                }],
            };
        }
    };

    if installed
        .lines()
        .any(|line| line.trim() == WASM_TARGET_TRIPLE)
    {
        Component {
            name: TARGET_COMPONENT.to_string(),
            status: ComponentStatus::Ok,
            summary: format!("{WASM_TARGET_TRIPLE} installed"),
            fix_commands: Vec::new(),
        }
    } else {
        Component {
            name: TARGET_COMPONENT.to_string(),
            status: ComponentStatus::Missing,
            summary: format!("{WASM_TARGET_TRIPLE} is not installed on the active toolchain"),
            fix_commands: vec![FixCommand {
                display: format!("rustup target add {WASM_TARGET_TRIPLE}"),
                program: "rustup".to_string(),
                args: vec![
                    "target".to_string(),
                    "add".to_string(),
                    WASM_TARGET_TRIPLE.to_string(),
                ],
                auto_runnable: true,
                doc_link: None,
            }],
        }
    }
}

/// `wasm-bindgen` present, and its version equal to the one the project pins.
fn bindgen_check(runner: &dyn ProcessRunner, project_dir: &Path) -> Component {
    let name = BINDGEN_COMPONENT.to_string();
    let declared = declared_bindgen_version(project_dir);
    let Some(installed) = installed_bindgen_version(runner) else {
        return Component {
            name,
            status: ComponentStatus::Missing,
            summary: "wasm-bindgen is not on PATH".to_string(),
            fix_commands: vec![install_bindgen_fix(declared.as_deref())],
        };
    };

    match declared {
        Some(declared) if declared == installed => Component {
            name,
            status: ComponentStatus::Ok,
            summary: format!("{installed}, matching the project's pin"),
            fix_commands: Vec::new(),
        },
        Some(declared) => Component {
            name,
            status: ComponentStatus::Missing,
            summary: format!(
                "installed {installed}, but the project pins {declared} — the CLI and the \
                 crate share a schema version and must be equal, so a build would fail after \
                 compiling"
            ),
            fix_commands: vec![install_bindgen_fix(Some(&declared))],
        },
        None => Component {
            name,
            status: ComponentStatus::Partial,
            summary: format!(
                "{installed} installed, but the project declares no wasm-bindgen dependency to \
                 match it against"
            ),
            fix_commands: Vec::new(),
        },
    }
}

fn install_bindgen_fix(version: Option<&str>) -> FixCommand {
    let mut args = vec![
        "install".to_string(),
        "-f".to_string(),
        "wasm-bindgen-cli".to_string(),
    ];
    if let Some(version) = version {
        args.push("--version".to_string());
        args.push(version.to_string());
    }
    FixCommand {
        display: format!("cargo {}", args.join(" ")),
        program: "cargo".to_string(),
        args,
        auto_runnable: true,
        doc_link: None,
    }
}

/// `wasm-opt` — optional by design, so its absence is never `Missing`.
fn wasm_opt_check(runner: &dyn ProcessRunner) -> Component {
    let name = WASM_OPT_COMPONENT.to_string();
    match runner.run("wasm-opt", &["--version"]) {
        Ok(out) if out.success => Component {
            name,
            status: ComponentStatus::Ok,
            summary: out
                .stdout
                .lines()
                .next()
                .unwrap_or("installed")
                .trim()
                .to_string(),
            fix_commands: Vec::new(),
        },
        _ => Component {
            name,
            status: ComponentStatus::Partial,
            summary: "not on PATH — builds succeed without it, the shipped .wasm is just larger"
                .to_string(),
            fix_commands: vec![FixCommand {
                display: "Install binaryen (provides wasm-opt)".to_string(),
                program: String::new(),
                args: Vec::new(),
                auto_runnable: false,
                doc_link: Some("https://github.com/WebAssembly/binaryen/releases".to_string()),
            }],
        },
    }
}

/// The host page a build would stage — the project's own `[web] host-dir`
/// when it carries both host-page files, else the framework's `platform/web`,
/// reachable through the project's `frust` path dependency. Mirrors
/// [`super::bundle::resolve_embedder`] exactly, so this row never disagrees
/// with what [`super::build`] itself would pick.
fn embedder_check(project_dir: &Path, web: &WebSection) -> Component {
    let name = EMBEDDER_COMPONENT.to_string();
    match bundle::resolve_embedder(project_dir, web) {
        Ok((dir, bundle::EmbedderSource::App)) => Component {
            name,
            status: ComponentStatus::Ok,
            summary: format!("the project's own host page at {}", dir.display()),
            fix_commands: Vec::new(),
        },
        Ok((dir, bundle::EmbedderSource::Framework)) => Component {
            name,
            status: ComponentStatus::Ok,
            summary: format!("no project host page — framework page at {}", dir.display()),
            fix_commands: Vec::new(),
        },
        Err(err) => Component {
            name,
            status: ComponentStatus::Missing,
            summary: format!("{err}"),
            fix_commands: Vec::new(),
        },
    }
}

/// Whether the host page [`embedder_check`] would stage carries a `?module=`
/// default that agrees with the `--out-name` this build would resolve —
/// [`super::bundle::verify_host_page_module`]'s check, surfaced here so a
/// mismatched `[web] out-name` shows up before a build is even attempted, not
/// only as [`super::WebBuildError::OutNameHostPageMismatch`] after one starts.
///
/// Nothing to check when the framework page would be staged (its `--out-name`
/// is always [`super::BINDGEN_OUT_NAME`], which is defined to match it) or
/// when no host page resolves at all ([`embedder_check`] already reports
/// that).
fn host_page_module_check(project_dir: &Path, web: &WebSection, app_name: &str) -> Component {
    let name = HOST_PAGE_COMPONENT.to_string();
    match bundle::resolve_embedder(project_dir, web) {
        Ok((_, bundle::EmbedderSource::Framework)) => Component {
            name,
            status: ComponentStatus::Ok,
            summary: "framework page in use — nothing to verify".to_string(),
            fix_commands: Vec::new(),
        },
        Ok((dir, bundle::EmbedderSource::App)) => {
            let out_name = web.out_name_or(app_name);
            match bundle::verify_host_page_module(&dir, out_name) {
                Ok(()) => Component {
                    name,
                    status: ComponentStatus::Ok,
                    summary: format!("matches out-name `{out_name}`"),
                    fix_commands: Vec::new(),
                },
                Err(err) => Component {
                    name,
                    status: ComponentStatus::Missing,
                    summary: format!("{err}"),
                    fix_commands: Vec::new(),
                },
            }
        }
        Err(_) => Component {
            name,
            status: ComponentStatus::Ok,
            summary: "no host page resolved yet — see the host page check above".to_string(),
            fix_commands: Vec::new(),
        },
    }
}

/// The `wasm-bindgen` version the project declares, from any dependency table
/// in its `Cargo.toml`.
///
/// Searched across `[dependencies]`, `[dev-dependencies]`,
/// `[build-dependencies]` and every `[target.'…'.dependencies]` table, because
/// a browser app's row is normally target-gated (`examples/web-gallery`'s is)
/// and a plain-table search would report "no pin" for exactly the projects
/// that have one. Accepts either the string shorthand (`wasm-bindgen =
/// "=0.2.128"`) or the table form (`{ version = "…" }`), and strips a leading
/// comparator so `=0.2.128`, `^0.2.128` and `0.2.128` all answer `0.2.128` —
/// this compares an exact version, not a requirement, so the comparator is
/// noise here even though the pin itself must stay exact.
fn declared_bindgen_version(project_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project_dir.join("Cargo.toml")).ok()?;
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;

    let mut tables: Vec<&dyn toml_edit::TableLike> = Vec::new();
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = doc.get(key).and_then(|item| item.as_table_like()) {
            tables.push(table);
        }
    }
    if let Some(targets) = doc.get("target").and_then(|item| item.as_table_like()) {
        for (_, cfg) in targets.iter() {
            if let Some(table) = cfg
                .as_table_like()
                .and_then(|cfg| cfg.get("dependencies"))
                .and_then(|item| item.as_table_like())
            {
                tables.push(table);
            }
        }
    }

    tables.iter().find_map(|table| {
        let item = table.get("wasm-bindgen")?;
        let raw = match item.as_str() {
            Some(version) => version,
            None => item.as_table_like()?.get("version")?.as_str()?,
        };
        Some(normalize_version(raw))
    })
}

/// Strips a leading requirement comparator and surrounding whitespace.
fn normalize_version(raw: &str) -> String {
    raw.trim()
        .trim_start_matches(['=', '^', '~', ' '])
        .to_string()
}

/// The installed CLI's version, from `wasm-bindgen --version`'s
/// `wasm-bindgen 0.2.128` line.
fn installed_bindgen_version(runner: &dyn ProcessRunner) -> Option<String> {
    let out = runner.run("wasm-bindgen", &["--version"]).ok()?;
    if !out.success {
        return None;
    }
    out.stdout
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-web-preflight-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A framework checkout with a complete embedder and an app pinning
    /// `wasm-bindgen` the way `examples/web-gallery` does — target-gated.
    fn checkout(tag: &str, manifest: &str) -> (PathBuf, PathBuf) {
        let root = temp_dir(tag);
        let embedder = root.join("platform/web");
        fs::create_dir_all(&embedder).unwrap();
        for file in ["index.html", "frust_web.js"] {
            fs::write(embedder.join(file), "// stub").unwrap();
        }
        fs::create_dir_all(root.join("crates/frust")).unwrap();
        let project = root.join("examples/app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("Cargo.toml"), manifest).unwrap();
        (root, project)
    }

    const PINNED_MANIFEST: &str = "[package]\nname = \"app\"\n\n\
         [dependencies]\nfrust = { path = \"../../crates/frust\" }\n\n\
         [target.'cfg(target_arch = \"wasm32\")'.dependencies]\n\
         wasm-bindgen = \"=0.2.128\"\n";

    fn healthy_runner() -> FakeProcessRunner {
        FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("aarch64-linux-android\nwasm32-unknown-unknown\nx86_64-unknown-linux-gnu\n"),
            )
            .with("wasm-bindgen --version", ok("wasm-bindgen 0.2.128\n"))
            .with("wasm-opt --version", ok("wasm-opt version 130\n"))
    }

    fn component<'a>(report: &'a WebPreflight, name: &str) -> &'a Component {
        report
            .components
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no `{name}` component in {report:?}"))
    }

    #[test]
    fn a_complete_host_reports_ok_and_is_ready() {
        let (root, project) = checkout("healthy", PINNED_MANIFEST);
        let report = preflight(&healthy_runner(), &project);
        assert_eq!(report.status(), ComponentStatus::Ok, "{report:?}");
        assert!(report.is_ready());
        assert!(report.blocking().is_empty());
        assert_eq!(report.components.len(), 6);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_wasm32_target_blocks_and_offers_the_rustup_command() {
        let (root, project) = checkout("no-target", PINNED_MANIFEST);
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("x86_64-unknown-linux-gnu\n"),
            )
            .with("wasm-bindgen --version", ok("wasm-bindgen 0.2.128\n"))
            .with("wasm-opt --version", ok("wasm-opt version 130\n"));
        let report = preflight(&runner, &project);
        let target = component(&report, TARGET_COMPONENT);
        assert_eq!(target.status, ComponentStatus::Missing);
        assert!(!report.is_ready());
        assert_eq!(
            target.fix_commands[0].args,
            vec!["target", "add", "wasm32-unknown-unknown"]
        );
        assert!(target.fix_commands[0].auto_runnable);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_rustup_at_all_is_missing_with_guidance_rather_than_a_command() {
        let (root, project) = checkout("no-rustup", PINNED_MANIFEST);
        let runner = FakeProcessRunner::new()
            .missing("rustup target list --installed")
            .with("wasm-bindgen --version", ok("wasm-bindgen 0.2.128\n"))
            .with("wasm-opt --version", ok("wasm-opt version 130\n"));
        let target = component(&preflight(&runner, &project), TARGET_COMPONENT).clone();
        assert_eq!(target.status, ComponentStatus::Missing);
        assert!(!target.fix_commands[0].auto_runnable);
        assert!(target.fix_commands[0].doc_link.is_some());
        let _ = fs::remove_dir_all(&root);
    }

    /// The decision this check exists for: a CLI one patch off the crate pin
    /// is a build that dies after a full release compile, so it blocks.
    #[test]
    fn a_bindgen_version_mismatch_blocks_and_names_both_versions() {
        let (root, project) = checkout("bindgen-mismatch", PINNED_MANIFEST);
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("wasm32-unknown-unknown\n"),
            )
            .with("wasm-bindgen --version", ok("wasm-bindgen 0.2.100\n"))
            .with("wasm-opt --version", ok("wasm-opt version 130\n"));
        let report = preflight(&runner, &project);
        let bindgen = component(&report, BINDGEN_COMPONENT);
        assert_eq!(bindgen.status, ComponentStatus::Missing);
        assert!(bindgen.summary.contains("0.2.100"), "{}", bindgen.summary);
        assert!(bindgen.summary.contains("0.2.128"), "{}", bindgen.summary);
        assert_eq!(
            bindgen.fix_commands[0].display,
            "cargo install -f wasm-bindgen-cli --version 0.2.128"
        );
        assert!(!report.is_ready());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_bindgen_cli_blocks() {
        let (root, project) = checkout("bindgen-missing", PINNED_MANIFEST);
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("wasm32-unknown-unknown\n"),
            )
            .missing("wasm-bindgen --version")
            .with("wasm-opt --version", ok("wasm-opt version 130\n"));
        let report = preflight(&runner, &project);
        assert_eq!(
            component(&report, BINDGEN_COMPONENT).status,
            ComponentStatus::Missing
        );
        assert!(!report.is_ready());
        let _ = fs::remove_dir_all(&root);
    }

    /// Present but unverifiable: honest `Partial`, not a green row that
    /// checked nothing.
    #[test]
    fn a_project_with_no_bindgen_pin_is_partial_not_ok() {
        let (root, project) = checkout(
            "bindgen-unpinned",
            "[package]\nname = \"app\"\n\n[dependencies]\nfrust = { path = \"../../crates/frust\" }\n",
        );
        let report = preflight(&healthy_runner(), &project);
        let bindgen = component(&report, BINDGEN_COMPONENT);
        assert_eq!(bindgen.status, ComponentStatus::Partial);
        assert!(report.is_ready(), "a partial row never blocks a build");
        assert_eq!(report.status(), ComponentStatus::Partial);
        let _ = fs::remove_dir_all(&root);
    }

    /// The optional tool's whole contract: absent is worse, never fatal.
    #[test]
    fn a_missing_wasm_opt_is_partial_and_never_blocks() {
        let (root, project) = checkout("no-wasm-opt", PINNED_MANIFEST);
        let runner = FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                ok("wasm32-unknown-unknown\n"),
            )
            .with("wasm-bindgen --version", ok("wasm-bindgen 0.2.128\n"))
            .missing("wasm-opt --version");
        let report = preflight(&runner, &project);
        assert_eq!(
            component(&report, WASM_OPT_COMPONENT).status,
            ComponentStatus::Partial
        );
        assert!(report.is_ready());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unreachable_embedder_blocks_and_explains_itself() {
        let dir = temp_dir("no-embedder");
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
        let report = preflight(&healthy_runner(), &dir);
        let embedder = component(&report, EMBEDDER_COMPONENT);
        assert_eq!(embedder.status, ComponentStatus::Missing);
        assert!(embedder.summary.contains("frust"), "{}", embedder.summary);
        assert!(!report.is_ready());
        assert_eq!(report.blocking().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// An app's own host page is preferred over the framework, and reported
    /// as such — the same precedence [`super::build`] follows.
    #[test]
    fn an_app_host_page_is_reported_ok_without_the_framework() {
        let (root, project) = checkout("app-page", PINNED_MANIFEST);
        let host = project.join("web");
        fs::create_dir_all(&host).unwrap();
        fs::write(host.join("index.html"), "|| \"./pkg/app.js\";").unwrap();
        fs::write(host.join("frust_web.js"), "// stub").unwrap();
        let report = preflight(&healthy_runner(), &project);
        let embedder = component(&report, EMBEDDER_COMPONENT);
        assert_eq!(embedder.status, ComponentStatus::Ok);
        assert!(
            embedder.summary.contains("project's own"),
            "{}",
            embedder.summary
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A mismatched `[web] out-name` against the app's page blocks, before a
    /// build is attempted.
    #[test]
    fn a_host_page_module_mismatch_blocks() {
        let (root, project) = checkout(
            "module-mismatch",
            "[package]\nname = \"app\"\n\n\
             [dependencies]\nfrust = { path = \"../../crates/frust\" }\n\n\
             [target.'cfg(target_arch = \"wasm32\")'.dependencies]\n\
             wasm-bindgen = \"=0.2.128\"\n",
        );
        fs::write(
            project.join("frust.toml"),
            "[app]\nname = \"app\"\norg = \"dev.f0x\"\n\n[web]\nout-name = \"renamed\"\n",
        )
        .unwrap();
        let host = project.join("web");
        fs::create_dir_all(&host).unwrap();
        fs::write(host.join("index.html"), "|| \"./pkg/app.js\";").unwrap();
        fs::write(host.join("frust_web.js"), "// stub").unwrap();
        let report = preflight(&healthy_runner(), &project);
        let module = component(&report, HOST_PAGE_COMPONENT);
        assert_eq!(module.status, ComponentStatus::Missing);
        assert!(module.summary.contains("renamed"), "{}", module.summary);
        assert!(!report.is_ready());
        let _ = fs::remove_dir_all(&root);
    }

    /// The framework page's fixed `--out-name` needs no verification.
    #[test]
    fn the_framework_page_needs_no_module_verification() {
        let (root, project) = checkout("framework-page", PINNED_MANIFEST);
        let report = preflight(&healthy_runner(), &project);
        let module = component(&report, HOST_PAGE_COMPONENT);
        assert_eq!(module.status, ComponentStatus::Ok);
        let _ = fs::remove_dir_all(&root);
    }

    /// No `frust.toml` at all is reported `Ok`, not `Missing` — this
    /// pipeline needs none.
    #[test]
    fn no_manifest_is_reported_ok() {
        let (root, project) = checkout("no-manifest", PINNED_MANIFEST);
        let report = preflight(&healthy_runner(), &project);
        let manifest = component(&report, MANIFEST_COMPONENT);
        assert_eq!(manifest.status, ComponentStatus::Ok);
        assert!(report.is_ready());
        let _ = fs::remove_dir_all(&root);
    }

    /// A malformed `frust.toml` blocks, rather than being silently treated as
    /// absent.
    #[test]
    fn a_malformed_manifest_blocks() {
        let (root, project) = checkout("malformed-manifest", PINNED_MANIFEST);
        fs::write(project.join("frust.toml"), "[web]\nout_name = \"x\"\n").unwrap();
        let report = preflight(&healthy_runner(), &project);
        let manifest = component(&report, MANIFEST_COMPONENT);
        assert_eq!(manifest.status, ComponentStatus::Missing);
        assert!(!report.is_ready());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_declared_version_is_read_from_every_dependency_table_shape() {
        for (tag, manifest, expected) in [
            (
                "plain-string",
                "[package]\nname=\"a\"\n[dependencies]\nwasm-bindgen = \"=0.2.128\"\n",
                Some("0.2.128"),
            ),
            (
                "table-form",
                "[package]\nname=\"a\"\n[dependencies]\nwasm-bindgen = { version = \"0.2.99\", features = [] }\n",
                Some("0.2.99"),
            ),
            (
                "target-gated",
                "[package]\nname=\"a\"\n[target.'cfg(target_arch = \"wasm32\")'.dependencies]\nwasm-bindgen = \"^0.2.90\"\n",
                Some("0.2.90"),
            ),
            ("absent", "[package]\nname=\"a\"\n", None),
        ] {
            let dir = temp_dir(tag);
            fs::write(dir.join("Cargo.toml"), manifest).unwrap();
            assert_eq!(declared_bindgen_version(&dir).as_deref(), expected, "{tag}");
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn the_installed_version_is_parsed_from_the_cli_banner() {
        let runner =
            FakeProcessRunner::new().with("wasm-bindgen --version", ok("wasm-bindgen 0.2.128\n"));
        assert_eq!(
            installed_bindgen_version(&runner).as_deref(),
            Some("0.2.128")
        );
    }
}
