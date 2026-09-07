//! Browser build assembly: `cargo build --target wasm32-unknown-unknown`,
//! `wasm-bindgen --target web`, an optional `wasm-opt` pass, and the servable
//! artifact directory the three of them produce — plus the dev server that
//! hands it to a browser and the preflight that says whether any of it can run.
//!
//! The web tier's counterpart to [`crate::desktop_build`], and shaped like it
//! on purpose: one entry point ([`build`]) that compiles through the injected
//! [`ProcessRunner`], lays out a directory under `[web] out-dir`, and returns
//! a typed [`WebBuildReport`] carrying the artifacts plus every non-fatal
//! observation as a [`WebBuildNote`].
//!
//! # `frust.toml`'s `[web]` section drives the pipeline, but is never required
//!
//! Unlike the desktop and mobile pipelines, this one does not require an app
//! manifest: there is no bundle identity to resolve, no icon to generate and
//! no launcher metadata to write, so a plain `wasm32` app crate with no
//! `frust.toml` at all is a valid input (`examples/web-gallery`'s own
//! shape — its own proof of this, see below). When a `frust.toml` **is**
//! present, its `[web]` section (`crate::manifest::WebSection`) governs every
//! layout decision below; an absent section behaves exactly like
//! [`WebSection::default`]. [`build`], [`preflight`] and
//! [`ServeOptions::from_manifest`] each read (or accept) the manifest for
//! this reason — see their own docs for exactly how.
//!
//! # The pipeline, and why the order is not negotiable
//!
//! 1. **Resolve the host page first, and verify it.** [`bundle::resolve_embedder`]
//!    picks between the project's own `<host-dir>` and the framework's
//!    `platform/web` (reached through the project's `frust` path dependency —
//!    see [`bundle`]) before anything is compiled, and
//!    [`bundle::verify_host_page_module`] refuses a resolved app page whose
//!    `?module=` default disagrees with the `wasm-bindgen --out-name` this
//!    build would use. Both checks run before the compile because a
//!    multi-minute release build that then fails on a missing or
//!    mismatched host page is the worst possible place to learn about it.
//! 2. **`cargo build --target wasm32-unknown-unknown`**, with the mode's
//!    profile flag.
//! 3. **`wasm-bindgen --target web`** over the produced `.wasm`, straight into
//!    the artifact directory's `pkg/`, named `--out-name` to match whichever
//!    host page step 1 resolved. This step is what turns a raw module into
//!    something a browser can `import` — the `.wasm` cargo emits is not
//!    loadable on its own.
//! 4. **`wasm-opt`, strictly after `wasm-bindgen`, never before.** Optimizing
//!    the pre-bindgen module strips or reorders the sections `wasm-bindgen`
//!    reads to generate its glue; the ordering is recorded as a finding in
//!    `examples/web-gallery/README.md` and is a correctness rule here, not a
//!    preference. Runs when [`WebSection::wasm_opt_enabled`] says so for this
//!    build's mode; optional either way, and skipped rather than fatal — see
//!    [`WebBuildNote`].
//! 5. **Stage the host page.** `index.html` and `frust_web.js` are copied
//!    verbatim from whichever directory step 1 resolved.
//!
//! # `--out-name`, resolved from the host page that gets staged
//!
//! Every host page resolves its app module from `?module=`, defaulting to
//! `./pkg/<name>.js` for its own `<name>`. There are exactly two names in
//! play, one per [`bundle::EmbedderSource`]:
//!
//! - **The app's own page** defaults to the project's own name
//!   (`templates/app/web.tmpl/`'s `web_module_name`), so this build's
//!   `--out-name` is `WebSection::out_name_or(&app_name)` — `[web] out-name`
//!   when set, else `[app] name` (or, with no `frust.toml` at all, the
//!   `Cargo.toml` package name — see [`bundle::package_name`]).
//! - **The framework's page** defaults to `app` and this pipeline never
//!   rewrites a file it does not own (see [`bundle`]'s module doc), so its
//!   `--out-name` is always [`BINDGEN_OUT_NAME`] regardless of `[web]
//!   out-name` — an explicit override is reported ignored
//!   ([`WebBuildNote::FrameworkOutNameIgnored`]).
//!
//! [`bundle::verify_host_page_module`] is what keeps the app-page branch
//! honest: overriding `out-name` without also editing the page's own default
//! is a typed refusal, not a build that quietly 404s in a browser.
//!
//! # Print-free
//!
//! Compile and tool output leaves through the `on_line` sink, and everything
//! else worth telling a human comes back as a [`WebBuildNote`]
//! (`docs/CODE_STANDARDS.md`'s printing anti-pattern,
//! `crates/frust-drive/tests/print_free_cores.rs`). The dev server takes its
//! own optional log sink for the same reason ([`RequestLog`]).

mod bundle;
mod preflight;
mod serve;

use std::fmt;
use std::path::{Path, PathBuf};

use crate::build_info::{BuildInfo, WASM_TARGET_TRIPLE};
use crate::doctor::{EnvLookup, RealEnv};
#[cfg(test)]
use crate::manifest::WebSection;
use crate::manifest::{self, Manifest};
use crate::process::{ProcessRunner, tail_lines};

pub use bundle::{artifact_dir, embedder_dir};
pub use preflight::{
    BINDGEN_COMPONENT, EMBEDDER_COMPONENT, HOST_PAGE_COMPONENT, MANIFEST_COMPONENT,
    TARGET_COMPONENT, WASM_OPT_COMPONENT, WebPreflight, preflight,
};
pub use serve::{
    CONTENT_TYPES, DEFAULT_CONTENT_TYPE, DEFAULT_PORT, DevServer, RequestLog, ServeError,
    ServeOptions, Status, content_type, serve,
};

/// The `wasm-bindgen --out-name` a build uses when the framework's
/// `platform/web` page is staged, matching that page's fixed
/// `./pkg/app.js` default — see the module doc's `--out-name` section.
pub const BINDGEN_OUT_NAME: &str = "app";

/// How many trailing output lines a failed tool invocation reports — the same
/// bound the desktop/Android/iOS pipelines use.
const FAILURE_TAIL_LINES: usize = 50;

/// What a successful browser build produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebBuildReport {
    /// The artifact directory: `<project>/<out-dir>` (`[web] out-dir`,
    /// default `build/web`). This is the directory to hand [`serve`], to
    /// upload, or to copy behind a CDN.
    pub root: PathBuf,
    /// The staged host page — the URL a browser opens.
    pub index_html: PathBuf,
    /// The `wasm-bindgen` JS glue `index.html` imports (`pkg/<out-name>.js`).
    pub module_js: PathBuf,
    /// The WebAssembly module that glue loads (`pkg/<out-name>_bg.wasm`).
    pub wasm: PathBuf,
    /// Every file in the artifact directory, `pkg/` contents first and the two
    /// staged host-page files last (write order).
    pub artifacts: Vec<PathBuf>,
    /// Everything worth telling a human that is not a failure.
    pub notes: Vec<WebBuildNote>,
}

/// A non-fatal observation about a build, returned rather than printed so each
/// front-end renders it its own way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebBuildNote {
    /// The project's own host page (`[web] host-dir`, default `web`) was
    /// staged.
    AppHostPageStaged,
    /// No app host page was found; the framework's `platform/web` was staged
    /// instead.
    FrameworkHostPageStaged,
    /// The framework page was staged, so an explicit `[web] out-name` was
    /// ignored: the framework page's `?module=` default is fixed to
    /// `pkg/app.js`, and this pipeline never rewrites a file it does not own
    /// (see the `bundle` module doc).
    FrameworkOutNameIgnored { out_name: String },
    /// `wasm-opt` did not run because this build's mode does not default it
    /// on (`BuildMode::wasm_opt_default` — release only) and no `[web]
    /// wasm-opt = true` override forced it. Deliberate: `wasm-opt` over an
    /// unoptimized multi-megabyte module costs far more time than it saves in
    /// an edit-reload loop, and a non-release artifact is never shipped.
    WasmOptSkippedForMode,
    /// `wasm-opt` did not run because `[web] wasm-opt = false` explicitly
    /// disabled it, overriding a mode (release) that would otherwise have run
    /// it.
    WasmOptDisabled,
    /// `wasm-opt` is not installed, so the module was left as `wasm-bindgen`
    /// emitted it.
    WasmOptUnavailable,
    /// `wasm-opt` ran and failed. The module `wasm-bindgen` emitted is kept
    /// and the build succeeds — the failed pass wrote to a separate file that
    /// was never moved over it.
    WasmOptFailed { tail: String },
    /// `wasm-opt` shrank the module from `before` to `after` bytes.
    WasmOptApplied { before: u64, after: u64 },
}

impl fmt::Display for WebBuildNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WebBuildNote::AppHostPageStaged => {
                write!(f, "staged the project's own host page")
            }
            WebBuildNote::FrameworkHostPageStaged => write!(
                f,
                "no project host page found — staged the framework's `platform/web` page instead"
            ),
            WebBuildNote::FrameworkOutNameIgnored { out_name } => write!(
                f,
                "`[web] out-name = \"{out_name}\"` was ignored: the framework page's `?module=` \
                 default is fixed to `pkg/app.js`, so this build used `--out-name app` instead"
            ),
            WebBuildNote::WasmOptSkippedForMode => write!(
                f,
                "this build mode does not run `wasm-opt` by default (it costs more than it saves \
                 in an edit-reload loop) — build with --release, or set `[web] wasm-opt = true` \
                 to force it here"
            ),
            WebBuildNote::WasmOptDisabled => write!(
                f,
                "`[web] wasm-opt = false` disabled the optimizer for this build — the shipped \
                 module is unoptimized by explicit choice"
            ),
            WebBuildNote::WasmOptUnavailable => write!(
                f,
                "`wasm-opt` is not on PATH — the module is unoptimized and larger than a release \
                 artifact should be. Install binaryen to shrink it"
            ),
            WebBuildNote::WasmOptFailed { tail } => write!(
                f,
                "`wasm-opt` failed; the unoptimized module was kept and the build succeeded:\n{tail}"
            ),
            WebBuildNote::WasmOptApplied { before, after } => write!(
                f,
                "`wasm-opt` shrank the module from {before} to {after} bytes"
            ),
        }
    }
}

/// Everything that can stop a browser build — a library-contract enum callers
/// match on (`docs/CODE_STANDARDS.md`'s Error Handling convention), since the
/// CLI and the TUI both branch on the missing-toolchain cases to point at
/// [`preflight`].
#[derive(Debug, thiserror::Error)]
pub enum WebBuildError {
    #[error("reading `frust.toml` at '{}': {reason}", project_dir.display())]
    Manifest {
        project_dir: PathBuf,
        reason: String,
    },
    #[error(
        "the project's `Cargo.toml` at '{}' declares no `frust = {{ path = ... }}` dependency, \
         so the browser embedder (`platform/web`) cannot be located — a browser build stages its \
         host page from the framework checkout the app is built against, when the project \
         supplies no host page of its own",
        manifest.display()
    )]
    NoFrustDependency { manifest: PathBuf },
    #[error(
        "the browser embedder at '{}' has no `{missing}` — that path is derived from this \
         project's `frust` dependency ('{frust_path}'), so a moved or incomplete framework \
         checkout is the usual cause",
        dir.display()
    )]
    EmbedderIncomplete {
        dir: PathBuf,
        missing: &'static str,
        frust_path: String,
    },
    #[error("reading `[package] name` from '{}': {reason}", manifest.display())]
    PackageName { manifest: PathBuf, reason: String },
    #[error(
        "the staged host page at '{}' defaults its `?module=` query to `pkg/{page_module}.js`, \
         but this build resolved `[web] out-name` to `{out_name}` — wasm-bindgen would write \
         `pkg/{out_name}.js`, which the page never loads by default; either drop the `out-name` \
         override or edit the page's `?module=` default to match",
        path.display()
    )]
    OutNameHostPageMismatch {
        path: PathBuf,
        page_module: String,
        out_name: String,
    },
    #[error(
        "refusing to prepare the artifact directory '{path}': it is not inside the project \
         directory '{project_dir}' — preparing it deletes it recursively first, so a target \
         outside the project's own tree is never touched; `[web] out-dir` must resolve to a \
         subdirectory of the project root"
    )]
    UnsafeArtifactDir { path: PathBuf, project_dir: PathBuf },
    #[error(
        "refusing to prepare the artifact directory '{artifact_dir}': it overlaps with the host \
         page directory '{host_page_dir}' or the project's src directory — preparing the \
         artifact directory deletes its contents, so a build would destroy the host page or \
         source code; configure `[web] out-dir` to a safe location that does not touch the \
         project's source or host page"
    )]
    ArtifactDirOverlapsSource {
        artifact_dir: PathBuf,
        host_page_dir: PathBuf,
    },
    #[error("spawning `cargo {args}`: {reason}")]
    CargoSpawn { args: String, reason: String },
    #[error("`cargo {args}` failed:\n{tail}")]
    CargoFailed { args: String, tail: String },
    #[error(
        "`cargo build --target {WASM_TARGET_TRIPLE}` succeeded but produced no `{profile}` module \
         for `{crate_name}` — looked for {} — check the app crate really builds a binary or \
         cdylib target named `{crate_name}`",
        searched.iter().map(|p| format!("'{}'", p.display())).collect::<Vec<_>>().join(" and ")
    )]
    WasmNotFound {
        crate_name: String,
        profile: &'static str,
        searched: Vec<PathBuf>,
    },
    #[error(
        "spawning `wasm-bindgen`: {reason} — run the browser preflight (`frust doctor`) to see \
         whether it is installed and version-matched"
    )]
    BindgenSpawn { reason: String },
    #[error("`wasm-bindgen --target web` failed:\n{tail}")]
    BindgenFailed { tail: String },
    #[error(
        "`wasm-bindgen` reported success but did not write '{}' — the CLI's output layout does \
         not match what this pipeline stages",
        path.display()
    )]
    BindgenOutputMissing { path: PathBuf },
    #[error("{action} '{path}': {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Builds the Frust project at `project_dir` for the browser, producing a
/// servable artifact directory at `<project>/<out-dir>` (`[web] out-dir`,
/// default `build/web`).
///
/// Loads the project's `frust.toml` itself (`crate::manifest::load_optional`)
/// — an absent manifest is not an error here (see the module doc); a present
/// but unreadable one is ([`WebBuildError::Manifest`]).
///
/// `info` is taken as given — the release default belongs to the caller,
/// exactly as it does for the desktop and mobile pipelines.
///
/// **Print-free**: tool output and progress lines go through `on_line`;
/// non-fatal observations come back as [`WebBuildReport::notes`].
pub fn build(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
) -> Result<WebBuildReport, WebBuildError> {
    build_with_env(runner, &RealEnv, project_dir, info, on_line)
}

/// The testable core of [`build`]: `env` is injected so `CARGO_TARGET_DIR`
/// resolution can be driven without touching the real process environment.
fn build_with_env(
    runner: &dyn ProcessRunner,
    env: &dyn EnvLookup,
    project_dir: &Path,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
) -> Result<WebBuildReport, WebBuildError> {
    let manifest = load_manifest(project_dir)?;
    let crate_name = bundle::package_name(project_dir)?;
    let app_name = manifest
        .as_ref()
        .map(|m| m.app.name.clone())
        .unwrap_or_else(|| crate_name.clone());
    let web = manifest
        .as_ref()
        .and_then(|m| m.web.clone())
        .unwrap_or_default();

    // Before the compile, never after: see the module doc's pipeline order.
    let (embedder, source) = bundle::resolve_embedder(project_dir, &web)?;
    let out_name = match source {
        bundle::EmbedderSource::App => web.out_name_or(&app_name).to_string(),
        bundle::EmbedderSource::Framework => BINDGEN_OUT_NAME.to_string(),
    };
    if source == bundle::EmbedderSource::App {
        bundle::verify_host_page_module(&embedder, &out_name)?;
    }

    cargo_build(runner, project_dir, info, on_line)?;
    let module = locate_wasm(runner, env, project_dir, info, &crate_name)?;

    let root = artifact_dir(project_dir, &web);
    bundle::prepare_dir(&root, project_dir, &embedder)?;
    let pkg_dir = root.join(bundle::PKG_DIR);

    wasm_bindgen(runner, project_dir, &module, &pkg_dir, &out_name, on_line)?;
    let module_js = pkg_dir.join(format!("{out_name}.js"));
    let wasm = pkg_dir.join(format!("{out_name}_bg.wasm"));
    for produced in [&module_js, &wasm] {
        if !produced.is_file() {
            return Err(WebBuildError::BindgenOutputMissing {
                path: produced.clone(),
            });
        }
    }

    let mut notes = Vec::new();
    notes.push(match source {
        bundle::EmbedderSource::App => WebBuildNote::AppHostPageStaged,
        bundle::EmbedderSource::Framework => WebBuildNote::FrameworkHostPageStaged,
    });
    if source == bundle::EmbedderSource::Framework
        && let Some(requested) = &web.out_name
    {
        notes.push(WebBuildNote::FrameworkOutNameIgnored {
            out_name: requested.clone(),
        });
    }

    if web.wasm_opt_enabled(info.mode.wasm_opt_default()) {
        run_wasm_opt(runner, project_dir, &wasm, on_line, &mut notes)?;
    } else {
        notes.push(if web.wasm_opt == Some(false) {
            WebBuildNote::WasmOptDisabled
        } else {
            WebBuildNote::WasmOptSkippedForMode
        });
    }

    let mut artifacts = bundle::pkg_artifacts(&pkg_dir)?;
    artifacts.extend(bundle::stage_embedder(&embedder, &root)?);

    Ok(WebBuildReport {
        index_html: root.join("index.html"),
        root,
        module_js,
        wasm,
        artifacts,
        notes,
    })
}

/// Reads the project's `frust.toml` when one exists, mapping a present-but-
/// unreadable manifest to a typed error and an absent one to `None` — the
/// same "optional input" contract [`preflight`] uses.
fn load_manifest(project_dir: &Path) -> Result<Option<Manifest>, WebBuildError> {
    manifest::load_optional(project_dir).map_err(|err| WebBuildError::Manifest {
        project_dir: project_dir.to_path_buf(),
        reason: format!("{err:#}"),
    })
}

/// Runs `cargo build --target wasm32-unknown-unknown` for `info`'s mode in
/// `project_dir`, streaming each line to `on_line` prefixed with `[cargo]`.
///
/// **No `--features`.** The desktop and mobile pipelines thread
/// [`BuildMode::cargo_features`] through, which selects
/// `frust/perf-trace`+`frust/devtools` for a debug build and the app's own
/// `lean` for a release one. None of those apply here: the devtools service is
/// a loopback TCP listener with no `wasm32` counterpart, and `lean` is a
/// feature a browser app crate is not expected to declare. Passing them would
/// hand cargo undeclared or uncompilable feature names on the one target that
/// cannot support them, so the mode contributes its profile flag and nothing
/// else. A `--features` passthrough is a front-end decision this funnel has no
/// parameter for, exactly as the desktop pipeline's own compile step has none.
fn cargo_build(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    on_line: &mut dyn FnMut(&str),
) -> Result<(), WebBuildError> {
    let mut args: Vec<String> = vec!["build".to_string()];
    for arg in info.mode.cargo_profile_arg() {
        args.push((*arg).to_string());
    }
    args.push("--target".to_string());
    args.push(WASM_TARGET_TRIPLE.to_string());
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let printable = args.join(" ");

    let mut prefixed = |line: &str| on_line(&format!("[cargo] {line}"));
    let out = runner
        .run_streaming("cargo", &argv, Some(project_dir), &[], &mut prefixed)
        .map_err(|err| WebBuildError::CargoSpawn {
            args: printable.clone(),
            reason: format!("{err:#}"),
        })?;
    if !out.success {
        return Err(WebBuildError::CargoFailed {
            args: printable,
            tail: failure_tail(&out.stderr, &out.stdout),
        });
    }
    Ok(())
}

/// The `.wasm` a successful compile produced, under
/// `<target-dir>/wasm32-unknown-unknown/<profile>/`.
///
/// Both spellings of the file name are tried, in [`wasm_file_stems`]'s order,
/// because which one cargo writes depends on the *target kind* rather than on
/// anything readable from the package name alone.
fn locate_wasm(
    runner: &dyn ProcessRunner,
    env: &dyn EnvLookup,
    project_dir: &Path,
    info: &BuildInfo,
    crate_name: &str,
) -> Result<PathBuf, WebBuildError> {
    let profile = info.mode.cargo_profile_dir();
    let dir = resolve_target_dir(runner, env, project_dir)
        .join(WASM_TARGET_TRIPLE)
        .join(profile);
    let searched: Vec<PathBuf> = wasm_file_stems(crate_name)
        .into_iter()
        .map(|stem| dir.join(format!("{stem}.wasm")))
        .collect();
    match searched.iter().find(|path| path.is_file()) {
        Some(path) => Ok(path.clone()),
        None => Err(WebBuildError::WasmNotFound {
            crate_name: crate_name.to_string(),
            profile,
            searched,
        }),
    }
}

/// The file-name stems a `wasm32` compile of `crate_name` can produce, most
/// likely first.
///
/// Cargo does **not** apply one rule here. A **binary** target's artifact is
/// named after the bin target, which defaults to the package name *exactly as
/// written* — `web-gallery` produces `web-gallery.wasm`, dash intact. A
/// **library** target (a `cdylib`, the shape a non-`bin` wasm crate ships) is
/// named after the crate name, where cargo normalizes `-` to `_`, producing
/// `web_gallery.wasm`. A package whose name carries no dash spells both the
/// same way and yields a single candidate.
///
/// The binary spelling comes first because `frust::web_app!` generates a
/// `main`: a Frust browser app is a binary crate, and the `cdylib` case is the
/// accommodation, not the default. Reading the kind out of `cargo metadata`
/// would be the exact answer, but it costs a second metadata invocation to
/// choose between two candidates that can be probed on disk for nothing.
fn wasm_file_stems(crate_name: &str) -> Vec<String> {
    let normalized = crate_name.replace('-', "_");
    if normalized == crate_name {
        vec![normalized]
    } else {
        vec![crate_name.to_string(), normalized]
    }
}

/// Where cargo writes artifacts for `project_dir`, in cargo's own precedence
/// order: `CARGO_TARGET_DIR` from the environment (relative values resolved
/// against the project directory), then whatever `cargo metadata` reports
/// (which folds in any `build.target-dir` from cargo's config files — a shared
/// global target directory is common, and assuming `<project>/target` silently
/// loses the module on such a machine), then `<project>/target`.
///
/// The same resolution `desktop_build::cargo` performs, restated rather than
/// shared: that function is private to its own module, and the alternative —
/// promoting it to a crate-visible helper — would edit a file this pipeline
/// has no other reason to touch. The invocation and the precedence order are
/// identical, so the two cannot disagree about where cargo writes.
fn resolve_target_dir(
    runner: &dyn ProcessRunner,
    env: &dyn EnvLookup,
    project_dir: &Path,
) -> PathBuf {
    if let Some(dir) = env.get("CARGO_TARGET_DIR").filter(|dir| !dir.is_empty()) {
        let dir = PathBuf::from(dir);
        return if dir.is_absolute() {
            dir
        } else {
            project_dir.join(dir)
        };
    }

    let manifest_path = project_dir.join("Cargo.toml");
    let manifest_path = manifest_path.to_string_lossy().to_string();
    let metadata = runner.run(
        "cargo",
        &[
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            &manifest_path,
        ],
    );
    if let Ok(out) = metadata
        && out.success
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&out.stdout)
        && let Some(dir) = value.get("target_directory").and_then(|d| d.as_str())
    {
        return PathBuf::from(dir);
    }

    project_dir.join("target")
}

/// Runs `wasm-bindgen --target web` over `module`, writing the glue and the
/// processed module straight into the artifact directory's `pkg/`, named
/// `out_name` — the resolved value from the module doc's `--out-name`
/// section.
///
/// `--target web` (not `bundler`, not `no-modules`) is what both host pages
/// require: it emits an ES module with a `default` export the page
/// `import()`s and hands to `frust_web.js`'s `mount()`.
fn wasm_bindgen(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    module: &Path,
    pkg_dir: &Path,
    out_name: &str,
    on_line: &mut dyn FnMut(&str),
) -> Result<(), WebBuildError> {
    bundle::create_dir(pkg_dir)?;
    let out_dir = pkg_dir.to_string_lossy().to_string();
    let module = module.to_string_lossy().to_string();
    let argv = vec![
        "--target",
        "web",
        "--out-dir",
        out_dir.as_str(),
        "--out-name",
        out_name,
        module.as_str(),
    ];

    let mut prefixed = |line: &str| on_line(&format!("[wasm-bindgen] {line}"));
    let out = runner
        .run_streaming("wasm-bindgen", &argv, Some(project_dir), &[], &mut prefixed)
        .map_err(|err| WebBuildError::BindgenSpawn {
            reason: format!("{err:#}"),
        })?;
    if !out.success {
        return Err(WebBuildError::BindgenFailed {
            tail: failure_tail(&out.stderr, &out.stdout),
        });
    }
    Ok(())
}

/// Optimizes `wasm` in place, or records why it did not run.
///
/// Runs **after** `wasm-bindgen` (the module doc's step 4). Whether it runs
/// at all is the caller's decision ([`WebSection::wasm_opt_enabled`]); this
/// function assumes it is wanted and only ever records why the *tool itself*
/// declined.
///
/// `-O --all-features` is the invocation `examples/web-gallery/README.md`
/// derived: bare `-O` refuses to parse the module this toolchain combination
/// produces, so the feature flag is not an optimization choice but the thing
/// that makes the tool accept the input at all.
///
/// Writes to a sibling `.opt.wasm` and renames it over the original only on
/// success, because `wasm-bindgen`'s generated glue imports the module by
/// its fixed `<out-name>_bg.wasm` name — the rename is how the optimized
/// module actually ships (the same two-step the README documents). A failure
/// therefore leaves the original untouched by construction.
///
/// Returns `Err` only for a filesystem failure of this pipeline's own making;
/// every `wasm-opt` outcome is a note.
fn run_wasm_opt(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    wasm: &Path,
    on_line: &mut dyn FnMut(&str),
    notes: &mut Vec<WebBuildNote>,
) -> Result<(), WebBuildError> {
    let optimized = wasm.with_extension("opt.wasm");
    let input = wasm.to_string_lossy().to_string();
    let output = optimized.to_string_lossy().to_string();
    let argv = vec![
        "-O",
        "--all-features",
        "-o",
        output.as_str(),
        input.as_str(),
    ];

    let mut prefixed = |line: &str| on_line(&format!("[wasm-opt] {line}"));
    let out = match runner.run_streaming("wasm-opt", &argv, Some(project_dir), &[], &mut prefixed) {
        Ok(out) => out,
        Err(_) => {
            notes.push(WebBuildNote::WasmOptUnavailable);
            return Ok(());
        }
    };
    if !out.success || !optimized.is_file() {
        notes.push(WebBuildNote::WasmOptFailed {
            tail: failure_tail(&out.stderr, &out.stdout),
        });
        let _ = std::fs::remove_file(&optimized);
        return Ok(());
    }

    let before = file_len(wasm);
    let after = file_len(&optimized);
    std::fs::rename(&optimized, wasm).map_err(|source| WebBuildError::Io {
        action: "replacing the module with its optimized form at",
        path: wasm.to_path_buf(),
        source,
    })?;
    notes.push(WebBuildNote::WasmOptApplied { before, after });
    Ok(())
}

/// A file's size, or `0` when it cannot be read — a size is reporting detail
/// on a note, never a reason to fail a build that already succeeded.
fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// The trailing output of a failed invocation: stderr when a tool wrote any,
/// else stdout — some of these tools report failures on each.
fn failure_tail(stderr: &str, stdout: &str) -> String {
    let tail = tail_lines(stderr, FAILURE_TAIL_LINES);
    if tail.is_empty() {
        tail_lines(stdout, FAILURE_TAIL_LINES)
    } else {
        tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::BuildArgs;
    use crate::build_info::BuildMode;
    use crate::doctor::FakeEnv;
    use crate::process::{FakeProcessRunner, Output, StreamHandle};
    use std::fs;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-web-build-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn info(mode: BuildMode) -> BuildInfo {
        BuildInfo::from_args(BuildArgs::default(), mode).unwrap()
    }

    /// How one scripted tool behaves in a [`PipelineRunner`] run.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Tool {
        /// Exits 0 and writes the files the real tool would have written.
        Works,
        /// Exits non-zero, writing nothing.
        Fails,
        /// Not on PATH — the spawn itself errors.
        Absent,
    }

    /// A `ProcessRunner` that also produces its tools' *files*.
    ///
    /// `FakeProcessRunner` scripts exit codes and output, which is the right
    /// shape for a pipeline that only reads what a tool printed — but every
    /// stage here consumes the previous stage's files, and the guarded rebuild
    /// deletes the artifact directory between them. A runner that reports
    /// success while writing nothing therefore cannot exercise the pipeline at
    /// all past its first stage. This one writes a plausible artifact for each
    /// invocation it accepts, so the ordering rules (bindgen before opt, the
    /// rebuild clearing the directory, the optimized module replacing the
    /// original) are observed on disk rather than asserted about argv.
    struct PipelineRunner {
        target_dir: PathBuf,
        profile: &'static str,
        module_stem: String,
        cargo: Tool,
        bindgen: Tool,
        wasm_opt: Tool,
        /// Every streamed invocation, `"<cmd> <args...>"`, in call order.
        invocations: Mutex<Vec<String>>,
        cwds: Mutex<Vec<Option<PathBuf>>>,
    }

    /// The bytes a scripted `wasm-bindgen` writes as the module, and what a
    /// scripted `wasm-opt` shrinks it to — distinct sizes so a note's numbers
    /// and a "was it replaced?" assertion mean something.
    const BINDGEN_WASM_LEN: usize = 64;
    const OPTIMIZED_WASM_LEN: usize = 40;

    impl PipelineRunner {
        fn new(target_dir: &Path, profile: &'static str, module_stem: &str) -> Self {
            Self {
                target_dir: target_dir.to_path_buf(),
                profile,
                module_stem: module_stem.to_string(),
                cargo: Tool::Works,
                bindgen: Tool::Works,
                wasm_opt: Tool::Works,
                invocations: Mutex::new(Vec::new()),
                cwds: Mutex::new(Vec::new()),
            }
        }

        fn cargo(mut self, tool: Tool) -> Self {
            self.cargo = tool;
            self
        }

        fn wasm_opt(mut self, tool: Tool) -> Self {
            self.wasm_opt = tool;
            self
        }

        fn invocations(&self) -> Vec<String> {
            self.invocations.lock().unwrap().clone()
        }

        fn ran(&self, program: &str) -> bool {
            self.invocations()
                .iter()
                .any(|call| call.starts_with(program))
        }

        /// The value following `flag` in an argv.
        fn flag_value(args: &[&str], flag: &str) -> PathBuf {
            let index = args
                .iter()
                .position(|arg| *arg == flag)
                .unwrap_or_else(|| panic!("no `{flag}` in {args:?}"));
            PathBuf::from(args[index + 1])
        }
    }

    fn failed(stderr: &str) -> anyhow::Result<Output> {
        Ok(Output {
            success: false,
            stdout: String::new(),
            stderr: stderr.to_string(),
        })
    }

    fn succeeded() -> anyhow::Result<Output> {
        Ok(Output {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    impl ProcessRunner for PipelineRunner {
        fn run(&self, cmd: &str, args: &[&str]) -> anyhow::Result<Output> {
            if cmd == "cargo" && args.first() == Some(&"metadata") {
                return Ok(Output {
                    success: true,
                    stdout: format!(
                        "{{\"packages\":[],\"target_directory\":\"{}\",\"version\":1}}",
                        self.target_dir.display()
                    ),
                    stderr: String::new(),
                });
            }
            anyhow::bail!("unscripted invocation: {cmd} {}", args.join(" "))
        }

        fn run_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            _env: &[(&str, &str)],
            on_line: &mut dyn FnMut(&str),
        ) -> anyhow::Result<Output> {
            self.invocations
                .lock()
                .unwrap()
                .push(format!("{cmd} {}", args.join(" ")));
            self.cwds.lock().unwrap().push(cwd.map(Path::to_path_buf));

            match cmd {
                "cargo" => match self.cargo {
                    Tool::Absent => anyhow::bail!("cargo: no such file or directory"),
                    Tool::Fails => failed("error[E0432]: unresolved import `frust::web`\n"),
                    Tool::Works => {
                        let dir = self.target_dir.join(WASM_TARGET_TRIPLE).join(self.profile);
                        fs::create_dir_all(&dir)?;
                        fs::write(
                            dir.join(format!("{}.wasm", self.module_stem)),
                            b"\0asm\x01\0\0\0",
                        )?;
                        on_line("Compiling web-app v0.1.0");
                        succeeded()
                    }
                },
                "wasm-bindgen" => match self.bindgen {
                    Tool::Absent => anyhow::bail!("wasm-bindgen: no such file or directory"),
                    Tool::Fails => failed("error: schema version mismatch\n"),
                    Tool::Works => {
                        let out_dir = Self::flag_value(args, "--out-dir");
                        let name = args[args
                            .iter()
                            .position(|arg| *arg == "--out-name")
                            .expect("--out-name")
                            + 1];
                        fs::create_dir_all(&out_dir)?;
                        fs::write(out_dir.join(format!("{name}.js")), "export default init;")?;
                        fs::write(out_dir.join(format!("{name}.d.ts")), "declare module;")?;
                        fs::write(
                            out_dir.join(format!("{name}_bg.wasm")),
                            vec![7u8; BINDGEN_WASM_LEN],
                        )?;
                        succeeded()
                    }
                },
                "wasm-opt" => match self.wasm_opt {
                    Tool::Absent => anyhow::bail!("wasm-opt: no such file or directory"),
                    Tool::Fails => failed("[parse exception] unexpected section\n"),
                    Tool::Works => {
                        fs::write(Self::flag_value(args, "-o"), vec![9u8; OPTIMIZED_WASM_LEN])?;
                        succeeded()
                    }
                },
                other => anyhow::bail!("unscripted streamed invocation: {other}"),
            }
        }

        fn spawn_streaming(
            &self,
            cmd: &str,
            _args: &[&str],
            _cwd: Option<&Path>,
            _env: &[(&str, &str)],
        ) -> anyhow::Result<StreamHandle> {
            anyhow::bail!("this pipeline never spawns a background stream (`{cmd}`)")
        }
    }

    /// A framework checkout plus an app crate, in the real relative shape,
    /// with **no** app-owned host page — every test that wants the framework
    /// page staged uses this as-is; tests that want the app's own page add a
    /// `web/` directory of their own.
    /// Returns `(checkout root, project dir, target dir)`.
    fn checkout(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = temp_dir(tag);
        let embedder = root.join("platform/web");
        fs::create_dir_all(&embedder).unwrap();
        fs::write(embedder.join("index.html"), "<!doctype html>").unwrap();
        fs::write(embedder.join("frust_web.js"), "export function mount() {}").unwrap();
        fs::create_dir_all(root.join("crates/frust")).unwrap();
        let project = root.join("examples/web-app");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"web-app\"\n\n[dependencies]\nfrust = { path = \"../../crates/frust\" }\n",
        )
        .unwrap();
        let target = root.join("shared-target");
        (root, project, target)
    }

    /// Adds the app's own host page (the `[web] host-dir` default, `web/`) to
    /// a project built by [`checkout`], with a `?module=` default naming
    /// `module_name`.
    fn add_app_host_page(project: &Path, module_name: &str) {
        let host = project.join("web");
        fs::create_dir_all(&host).unwrap();
        fs::write(
            host.join("index.html"),
            format!("|| \"./pkg/{module_name}.js\";"),
        )
        .unwrap();
        fs::write(host.join("frust_web.js"), "export function mount() {}").unwrap();
    }

    fn env_for(target: &Path) -> FakeEnv {
        FakeEnv::new().set("CARGO_TARGET_DIR", target.to_str().unwrap())
    }

    /// The acceptance criterion in one test: a library-level call with no app
    /// host page and no `frust.toml` produces a servable artifact directory
    /// through the framework fallback — `examples/web-gallery`'s own shape.
    #[test]
    fn a_release_build_with_no_app_page_falls_back_to_the_framework() {
        let (root, project, target) = checkout("release-e2e");
        let runner = PipelineRunner::new(&target, "release", "web_app");
        let mut lines = Vec::new();
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();

        assert_eq!(report.root, project.join("build/web"));
        assert!(report.index_html.is_file(), "the host page must be staged");
        assert!(project.join("build/web/frust_web.js").is_file());
        assert_eq!(report.module_js, project.join("build/web/pkg/app.js"));
        assert_eq!(report.wasm, project.join("build/web/pkg/app_bg.wasm"));
        assert!(report.wasm.is_file());
        // The staged page is the embedder's own bytes — never rewritten.
        assert_eq!(
            fs::read_to_string(&report.index_html).unwrap(),
            "<!doctype html>"
        );
        // Everything a browser needs, `pkg/` first and the host page last.
        let names: Vec<String> = report
            .artifacts
            .iter()
            .map(|p| {
                p.strip_prefix(&report.root)
                    .unwrap()
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "pkg/app.d.ts",
                "pkg/app.js",
                "pkg/app_bg.wasm",
                "index.html",
                "frust_web.js",
            ]
        );
        assert!(
            report
                .notes
                .contains(&WebBuildNote::FrameworkHostPageStaged)
        );
        // Compile output reaches the caller's sink, prefixed by stage.
        assert_eq!(lines, vec!["[cargo] Compiling web-app v0.1.0".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    /// The precedence's headline case: an app that supplies its own host page
    /// gets it staged, named after its own module — never the framework's.
    #[test]
    fn a_release_build_with_an_app_page_stages_it_and_names_the_module_after_the_project() {
        let (root, project, target) = checkout("app-page");
        add_app_host_page(&project, "web-app");
        let runner = PipelineRunner::new(&target, "release", "web_app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&report.index_html).unwrap(),
            "|| \"./pkg/web-app.js\";".to_string()
        );
        assert_eq!(report.module_js, project.join("build/web/pkg/web-app.js"));
        assert!(report.notes.contains(&WebBuildNote::AppHostPageStaged));
        assert!(
            runner
                .invocations()
                .iter()
                .any(|call| call.contains("--out-name web-app"))
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// `[web] out-name` overrides the app page's default module name.
    #[test]
    fn an_out_name_override_is_used_when_the_app_page_agrees() {
        let (root, project, target) = checkout("out-name-override");
        add_app_host_page(&project, "bundle");
        fs::write(
            project.join("frust.toml"),
            "[app]\nname = \"web-app\"\norg = \"dev.f0x\"\n\n[web]\nout-name = \"bundle\"\n",
        )
        .unwrap();
        let runner = PipelineRunner::new(&target, "release", "web_app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(report.module_js, project.join("build/web/pkg/bundle.js"));
        let _ = fs::remove_dir_all(&root);
    }

    /// A user who overrides `out-name` without updating their page's
    /// `?module=` default gets a typed refusal naming both values, before
    /// anything is compiled.
    #[test]
    fn an_out_name_override_that_disagrees_with_the_page_is_refused_before_the_compile() {
        let (root, project, target) = checkout("out-name-mismatch");
        add_app_host_page(&project, "web-app");
        fs::write(
            project.join("frust.toml"),
            "[app]\nname = \"web-app\"\norg = \"dev.f0x\"\n\n[web]\nout-name = \"renamed\"\n",
        )
        .unwrap();
        let runner = PipelineRunner::new(&target, "release", "web_app");
        let err = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("web-app"), "{message}");
        assert!(message.contains("renamed"), "{message}");
        assert!(matches!(err, WebBuildError::OutNameHostPageMismatch { .. }));
        assert!(
            runner.invocations().is_empty(),
            "nothing may be compiled before the mismatch is caught"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The framework page's `--out-name` is always `app`, and an override in
    /// this scenario is reported ignored rather than silently dropped.
    #[test]
    fn an_out_name_override_is_ignored_and_noted_when_the_framework_page_is_staged() {
        let (root, project, target) = checkout("framework-out-name-ignored");
        fs::write(
            project.join("frust.toml"),
            "[app]\nname = \"web-app\"\norg = \"dev.f0x\"\n\n[web]\nout-name = \"bundle\"\n",
        )
        .unwrap();
        let runner = PipelineRunner::new(&target, "release", "web_app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(report.module_js, project.join("build/web/pkg/app.js"));
        assert!(
            report
                .notes
                .contains(&WebBuildNote::FrameworkOutNameIgnored {
                    out_name: "bundle".to_string()
                })
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// `[web] out-dir` replaces the artifact root.
    #[test]
    fn an_out_dir_override_relocates_the_artifact_directory() {
        let (root, project, target) = checkout("out-dir-override");
        fs::write(
            project.join("frust.toml"),
            "[app]\nname = \"web-app\"\norg = \"dev.f0x\"\n\n[web]\nout-dir = \"public\"\n",
        )
        .unwrap();
        let runner = PipelineRunner::new(&target, "release", "web_app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(report.root, project.join("public"));
        assert!(!project.join("build/web").exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// The pipeline order, read off the invocation log: compile, then bindgen,
    /// then opt. `wasm-opt` before `wasm-bindgen` is a correctness bug, not a
    /// style question — see the module doc.
    #[test]
    fn the_stages_run_in_order_with_wasm_opt_strictly_after_bindgen() {
        let (root, project, target) = checkout("stage-order");
        let runner = PipelineRunner::new(&target, "release", "web_app");
        build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        let programs: Vec<String> = runner
            .invocations()
            .iter()
            .map(|call| call.split(' ').next().unwrap().to_string())
            .collect();
        assert_eq!(programs, vec!["cargo", "wasm-bindgen", "wasm-opt"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// The exact argv each stage carries — the contract with three host tools.
    #[test]
    fn each_stage_carries_the_argv_the_recipe_documents() {
        let (root, project, target) = checkout("argv");
        let runner = PipelineRunner::new(&target, "release", "web_app");
        build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        let calls = runner.invocations();
        assert_eq!(
            calls[0],
            format!("cargo build --release --target {WASM_TARGET_TRIPLE}")
        );
        assert!(
            calls[1].starts_with("wasm-bindgen --target web --out-dir "),
            "{}",
            calls[1]
        );
        // `--out-name app` is what makes the framework page's `?module=`
        // default resolve with no query string — the module doc's decision.
        assert!(calls[1].contains(" --out-name app "), "{}", calls[1]);
        assert!(
            calls[1].ends_with(&format!(
                "{}",
                target
                    .join(WASM_TARGET_TRIPLE)
                    .join("release/web_app.wasm")
                    .display()
            )),
            "{}",
            calls[1]
        );
        // `--all-features` is what makes wasm-opt accept this toolchain's
        // output at all, not an optimization preference.
        assert!(
            calls[2].starts_with("wasm-opt -O --all-features -o "),
            "{}",
            calls[2]
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// No `--features`: the mode's own feature selection is native/mobile
    /// vocabulary with no `wasm32` counterpart.
    #[test]
    fn no_mode_features_are_passed_to_a_browser_compile() {
        let (root, project, target) = checkout("no-features");
        for mode in [BuildMode::Debug, BuildMode::Profile, BuildMode::Release] {
            let runner = PipelineRunner::new(&target, mode.cargo_profile_dir(), "web_app");
            build_with_env(
                &runner,
                &env_for(&target),
                &project,
                &info(mode),
                &mut |_| {},
            )
            .unwrap();
            let compile = runner.invocations()[0].clone();
            assert!(!compile.contains("--features"), "{mode:?}: {compile}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// The compile runs in the app's own directory — load-bearing for a
    /// standalone workspace, which resolves nothing from a parent's lockfile.
    #[test]
    fn every_tool_runs_in_the_project_directory() {
        let (root, project, target) = checkout("cwd");
        let runner = PipelineRunner::new(&target, "release", "web_app");
        build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        let cwds = runner.cwds.lock().unwrap().clone();
        assert_eq!(cwds, vec![Some(project.clone()); 3]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_release_build_reports_the_size_wasm_opt_saved() {
        let (root, project, target) = checkout("opt-note");
        let runner = PipelineRunner::new(&target, "release", "web_app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        assert!(report.notes.contains(&WebBuildNote::WasmOptApplied {
            before: BINDGEN_WASM_LEN as u64,
            after: OPTIMIZED_WASM_LEN as u64,
        }));
        // The optimized module really replaced the original, under the fixed
        // name the generated glue imports.
        assert_eq!(
            fs::metadata(&report.wasm).unwrap().len(),
            OPTIMIZED_WASM_LEN as u64
        );
        assert!(
            !report.wasm.with_extension("opt.wasm").exists(),
            "the intermediate must not be left in the artifact directory"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A debug build skips the optimizer and says so, following the mode's
    /// own default with no `[web] wasm-opt` override.
    #[test]
    fn a_debug_build_skips_wasm_opt_with_a_note() {
        let (root, project, target) = checkout("debug-note");
        let runner = PipelineRunner::new(&target, "debug", "web_app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Debug),
            &mut |_| {},
        )
        .unwrap();
        assert!(report.notes.contains(&WebBuildNote::WasmOptSkippedForMode));
        assert!(!runner.ran("wasm-opt"), "{:?}", runner.invocations());
        assert!(report.wasm.is_file());
        let _ = fs::remove_dir_all(&root);
    }

    /// A profile build follows the same mode default a debug build does — no
    /// override means no `wasm-opt`. An earlier version special-cased only
    /// `BuildMode::Debug`, leaving this case to run the optimizer unnecessarily.
    #[test]
    fn a_profile_build_also_skips_wasm_opt_by_default() {
        let (root, project, target) = checkout("profile-note");
        let runner = PipelineRunner::new(&target, "profile", "web_app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Profile),
            &mut |_| {},
        )
        .unwrap();
        assert!(report.notes.contains(&WebBuildNote::WasmOptSkippedForMode));
        assert!(!runner.ran("wasm-opt"), "{:?}", runner.invocations());
        let _ = fs::remove_dir_all(&root);
    }

    /// `[web] wasm-opt = true` forces the optimizer on for a mode that would
    /// otherwise skip it.
    #[test]
    fn wasm_opt_true_forces_it_on_for_a_debug_build() {
        let (root, project, target) = checkout("wasm-opt-forced-on");
        fs::write(
            project.join("frust.toml"),
            "[app]\nname = \"web-app\"\norg = \"dev.f0x\"\n\n[web]\nwasm-opt = true\n",
        )
        .unwrap();
        let runner = PipelineRunner::new(&target, "debug", "web_app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Debug),
            &mut |_| {},
        )
        .unwrap();
        assert!(runner.ran("wasm-opt"));
        assert!(report.notes.contains(&WebBuildNote::WasmOptApplied {
            before: BINDGEN_WASM_LEN as u64,
            after: OPTIMIZED_WASM_LEN as u64,
        }));
        let _ = fs::remove_dir_all(&root);
    }

    /// `[web] wasm-opt = false` forces it off for a release build, and the
    /// note says so rather than reusing the mode-default wording.
    #[test]
    fn wasm_opt_false_disables_it_for_a_release_build() {
        let (root, project, target) = checkout("wasm-opt-forced-off");
        fs::write(
            project.join("frust.toml"),
            "[app]\nname = \"web-app\"\norg = \"dev.f0x\"\n\n[web]\nwasm-opt = false\n",
        )
        .unwrap();
        let runner = PipelineRunner::new(&target, "release", "web_app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        assert!(!runner.ran("wasm-opt"));
        assert!(report.notes.contains(&WebBuildNote::WasmOptDisabled));
        let _ = fs::remove_dir_all(&root);
    }

    /// A missing optimizer degrades the artifact, never the build.
    #[test]
    fn an_absent_wasm_opt_is_a_note_and_the_build_still_succeeds() {
        let (root, project, target) = checkout("no-wasm-opt");
        let runner = PipelineRunner::new(&target, "release", "web_app").wasm_opt(Tool::Absent);
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        assert!(report.notes.contains(&WebBuildNote::WasmOptUnavailable));
        assert_eq!(
            fs::metadata(&report.wasm).unwrap().len(),
            BINDGEN_WASM_LEN as u64,
            "the unoptimized module is kept"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A failing optimizer keeps the module `wasm-bindgen` emitted, byte for
    /// byte — the rename-on-success design, observed rather than asserted of.
    #[test]
    fn a_failing_wasm_opt_keeps_the_original_module() {
        let (root, project, target) = checkout("wasm-opt-fail");
        let runner = PipelineRunner::new(&target, "release", "web_app").wasm_opt(Tool::Fails);
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        assert!(
            report
                .notes
                .iter()
                .any(|n| matches!(n, WebBuildNote::WasmOptFailed { tail } if tail.contains("parse exception"))),
            "{:?}",
            report.notes
        );
        assert_eq!(
            fs::read(&report.wasm).unwrap(),
            vec![7u8; BINDGEN_WASM_LEN],
            "the module wasm-bindgen wrote must survive a failed pass"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// Fail before the compile, not after it — the module doc's first rule.
    #[test]
    fn a_missing_embedder_refuses_before_cargo_is_invoked() {
        let project = temp_dir("no-embedder");
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"a\"\n\n[dependencies]\nfrust = { path = \"nowhere\" }\n",
        )
        .unwrap();
        let runner = PipelineRunner::new(&project.join("target"), "release", "a");
        let err = build_with_env(
            &runner,
            &FakeEnv::new(),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(err, WebBuildError::EmbedderIncomplete { .. }),
            "{err:?}"
        );
        assert!(
            runner.invocations().is_empty(),
            "nothing may be compiled before the embedder is known to exist"
        );
        let _ = fs::remove_dir_all(&project);
    }

    /// A malformed `frust.toml` is a typed error, not a silent fall-through
    /// to the defaults.
    #[test]
    fn a_malformed_manifest_is_a_typed_error() {
        let (root, project, _target) = checkout("bad-manifest");
        fs::write(project.join("frust.toml"), "[web]\nout_name = \"x\"\n").unwrap();
        let runner = PipelineRunner::new(&root.join("target"), "release", "web_app");
        let err = build_with_env(
            &runner,
            &FakeEnv::new(),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, WebBuildError::Manifest { .. }), "{err:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_compile_reports_the_tail_of_its_output() {
        let (root, project, target) = checkout("cargo-fail");
        let runner = PipelineRunner::new(&target, "release", "web_app").cargo(Tool::Fails);
        let err = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(&err, WebBuildError::CargoFailed { tail, .. } if tail.contains("E0432")),
            "{err:?}"
        );
        assert!(
            !project.join("build/web").exists(),
            "a failed compile must not leave an artifact directory behind"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cargo_that_cannot_be_spawned_is_a_typed_error() {
        let (root, project, target) = checkout("cargo-absent");
        let runner = PipelineRunner::new(&target, "release", "web_app").cargo(Tool::Absent);
        let err = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, WebBuildError::CargoSpawn { .. }), "{err:?}");
        let _ = fs::remove_dir_all(&root);
    }

    /// A compile that succeeds without producing the module the pipeline then
    /// looks for names the exact path it looked at.
    #[test]
    fn a_missing_module_after_a_green_compile_is_a_typed_error() {
        let (root, project, target) = checkout("wasm-missing");
        // The scripted compile writes a module under a name the project does
        // not have, so neither candidate spelling exists.
        let runner = PipelineRunner::new(&target, "release", "other_name");
        let err = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(&err, WebBuildError::WasmNotFound { searched, crate_name, .. }
                if crate_name == "web-app"
                    && searched.len() == 2
                    && searched[0].ends_with("release/web-app.wasm")
                    && searched[1].ends_with("release/web_app.wasm")),
            "{err:?}"
        );
        // Both spellings appear in the message a user reads.
        let message = err.to_string();
        assert!(message.contains("web-app.wasm"), "{message}");
        assert!(message.contains("web_app.wasm"), "{message}");
        let _ = fs::remove_dir_all(&root);
    }

    /// A binary target keeps the package name's dash; a `cdylib` is
    /// normalized. Both are probed, binary first.
    #[test]
    fn both_cargo_artifact_spellings_are_candidates_binary_first() {
        assert_eq!(
            wasm_file_stems("web-gallery"),
            vec!["web-gallery".to_string(), "web_gallery".to_string()]
        );
        assert_eq!(
            wasm_file_stems("webgallery"),
            vec!["webgallery".to_string()]
        );
    }

    /// The real shape `examples/web-gallery` produces: a bin target whose
    /// artifact keeps the dash, found without a `cargo metadata` round trip.
    #[test]
    fn a_binary_targets_dashed_artifact_is_found() {
        let (root, project, target) = checkout("dashed-artifact");
        // The scripted compile writes exactly what a bin target writes.
        let runner = PipelineRunner::new(&target, "release", "web-app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        assert!(report.wasm.is_file());
        assert!(
            runner.invocations()[1].ends_with("release/web-app.wasm"),
            "{}",
            runner.invocations()[1]
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The shared-`build.target-dir` machine: the answer is cargo's, never
    /// `<project>/target`.
    #[test]
    fn the_target_dir_follows_cargos_own_precedence() {
        let dir = temp_dir("target-dir");
        let metadata = FakeProcessRunner::new().with(
            "cargo metadata --no-deps --format-version 1 --manifest-path",
            Output {
                success: true,
                stdout:
                    "{\"packages\":[],\"target_directory\":\"/data/cache/target\",\"version\":1}"
                        .to_string(),
                stderr: String::new(),
            },
        );
        assert_eq!(
            resolve_target_dir(&metadata, &FakeEnv::new(), &dir),
            PathBuf::from("/data/cache/target")
        );
        assert_eq!(
            resolve_target_dir(
                &metadata,
                &FakeEnv::new().set("CARGO_TARGET_DIR", "/elsewhere"),
                &dir
            ),
            PathBuf::from("/elsewhere")
        );
        assert_eq!(
            resolve_target_dir(&FakeProcessRunner::new(), &FakeEnv::new(), &dir),
            dir.join("target")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A rebuild must not serve a previous run's files.
    #[test]
    fn a_rebuild_clears_the_previous_artifact_directory() {
        let (root, project, target) = checkout("rebuild");
        let stale = artifact_dir(&project, &WebSection::default()).join("pkg/web_app.js");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, "previous build").unwrap();
        let runner = PipelineRunner::new(&target, "release", "web_app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();
        assert!(!stale.exists(), "a stale module survived the rebuild");
        assert!(report.wasm.is_file());
        let _ = fs::remove_dir_all(&root);
    }

    /// The artifact directory a build produces is the one [`serve`] is
    /// pointed at, and it answers the host page — the two halves of this
    /// module meeting on a real socket.
    #[test]
    fn the_artifact_directory_a_build_produces_is_servable() {
        let (root, project, target) = checkout("serve-built");
        let runner = PipelineRunner::new(&target, "release", "web_app");
        let report = build_with_env(
            &runner,
            &env_for(&target),
            &project,
            &info(BuildMode::Release),
            &mut |_| {},
        )
        .unwrap();

        let server = serve(
            &report.root,
            ServeOptions {
                port: 0,
                ..ServeOptions::default()
            },
            None,
        )
        .unwrap();
        let response = {
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(server.local_addr()).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            stream
                .write_all(b"GET /pkg/app_bg.wasm HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .unwrap();
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).unwrap();
            String::from_utf8_lossy(&buf).to_string()
        };
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(
            response.contains("Content-Type: application/wasm\r\n"),
            "{response}"
        );
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    /// Notes render as sentences a front-end can print unchanged.
    #[test]
    fn every_note_renders_a_human_message() {
        for note in [
            WebBuildNote::AppHostPageStaged,
            WebBuildNote::FrameworkHostPageStaged,
            WebBuildNote::FrameworkOutNameIgnored {
                out_name: "bundle".to_string(),
            },
            WebBuildNote::WasmOptSkippedForMode,
            WebBuildNote::WasmOptDisabled,
            WebBuildNote::WasmOptUnavailable,
            WebBuildNote::WasmOptFailed {
                tail: "boom".to_string(),
            },
            WebBuildNote::WasmOptApplied {
                before: 100,
                after: 90,
            },
        ] {
            assert!(!note.to_string().is_empty(), "{note:?}");
        }
    }
}
