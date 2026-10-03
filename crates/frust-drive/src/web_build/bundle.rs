//! Assembly primitives for the browser artifact directory: where it lives,
//! which host page it stages, the guarded rebuild of it, and the staging of
//! that host page over a `wasm-bindgen` build's own output.
//!
//! The web tier's counterpart to [`crate::desktop_build::bundle`], and
//! deliberately much smaller: a browser "bundle" is a directory a static file
//! server can hand out, with no per-OS layout, no icon containers and no
//! signature. What it *does* share with the desktop layouts is the dangerous
//! primitive — a rebuild removes the previous artifact directory recursively —
//! so [`prepare_dir`] carries the same containment guard, positioned at the
//! delete itself rather than at the caller.
//!
//! # Two embedders, resolved in priority order
//!
//! A build stages one of two host pages, decided by [`resolve_embedder`]:
//!
//! 1. **The app's own page**, at `<project>/<host-dir>` (`[web] host-dir`,
//!    default `web` — the exact directory `crates/frust-drive/templates/app/web.tmpl/` renders
//!    to). Used when both [`EMBEDDER_FILES`] exist there.
//! 2. **The framework's page**, otherwise: `platform/web` inside the
//!    `frust-shell-web` package, wherever cargo resolves that package in the
//!    project's dependency graph ([`crate::packages`]) — the checkout's
//!    `crates/frust-shell-web` under a `frust` path dependency, the unpacked
//!    crate in cargo's registry cache under a crates.io version. Nothing here
//!    reads the `frust` dependency itself, so both forms take the same route.
//!
//! An app whose framework checkout has moved, or whose registry crates cargo
//! cannot fetch, has no framework page to fall back to; the typed error
//! carries cargo's own explanation. A project that supplies its own host page
//! never needs the fallback at all.
//!
//! # Staged, not templated
//!
//! Both embedder files are copied **verbatim**, from whichever directory
//! [`resolve_embedder`] picked. Each host page resolves its app module from
//! `?module=`, defaulting to `./pkg/<name>.js` for its own `<name>` — the
//! app's page defaults to its project name (`crates/frust-drive/templates/app/web.tmpl/`'s
//! `web_module_name`), the framework's to `app`
//! ([`super::BINDGEN_OUT_NAME`]). Nothing in either page is rewritten; the
//! pipeline instead names its `wasm-bindgen` output to match whichever page
//! it staged, and refuses the build rather than silently drifting when it
//! cannot (see [`verify_host_page_module`]).

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::manifest::WebSection;
use crate::packages::{self, PackageLocator};

use super::WebBuildError;

/// The artifact-directory subdirectory `wasm-bindgen` output lands in, fixed
/// by both host pages' own `./pkg/<name>.js` shape.
pub(super) const PKG_DIR: &str = "pkg";

/// The package that ships the framework's host page.
pub(super) const EMBEDDER_PACKAGE: &str = "frust-shell-web";

/// The host page's directory inside [`EMBEDDER_PACKAGE`].
const EMBEDDER_PACKAGE_REL: &str = "platform/web";

/// The host-page files every embedder — the app's own or the framework's —
/// must carry, in copy order. Both are required: `index.html` imports
/// `./frust_web.js` directly, so a directory carrying only the first serves a
/// page that fails to boot.
pub(super) const EMBEDDER_FILES: &[&str] = &["index.html", "frust_web.js"];

/// Where a browser build for `project_dir` lands: `<project>/<out-dir>`
/// (`[web] out-dir`, default `build/web` — see
/// [`WebSection::out_dir_or_default`]).
///
/// Pure path arithmetic — nothing is created or checked. Public so a
/// front-end can name the directory (a "serving …" line, a `--open` URL)
/// without re-deriving the layout, and so [`serve`](super::serve) has an
/// obvious default root to be pointed at. Takes the resolved section (rather
/// than a whole `Manifest`) so a caller that already read one section out of
/// an optional manifest is not made to reconstruct one.
pub fn artifact_dir(project_dir: &Path, web: &WebSection) -> PathBuf {
    project_dir.join(web.out_dir_or_default())
}

/// Which host page a build staged — [`resolve_embedder`]'s answer, and the
/// deciding fact behind a build's `wasm-bindgen --out-name` and every note
/// [`super::build`] attaches about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EmbedderSource {
    /// The project's own host page, at `<project>/<host-dir>`.
    App,
    /// The framework's `crates/frust-shell-web/platform/web`, staged because the project supplies
    /// no host page of its own.
    Framework,
}

/// Picks the host page a build stages: the project's own `<host-dir>` when it
/// carries both [`EMBEDDER_FILES`], the framework's `crates/frust-shell-web/platform/web` otherwise.
///
/// Checked in that order and cheaply (two [`Path::is_file`] probes) — no
/// partial-app-page case exists the way [`embedder_dir`]'s typed refusal
/// exists for the framework page: an app that starts one of its own two files
/// but not the other is read as "no app page here", and the framework page is
/// tried next rather than the build failing on an incomplete `<host-dir>` a
/// project may never have meant to populate.
///
/// `locator` answers where `frust-shell-web` lives, and is asked only when
/// the project supplies no page of its own.
pub(super) fn resolve_embedder(
    project_dir: &Path,
    web: &WebSection,
    locator: &dyn PackageLocator,
) -> Result<(PathBuf, EmbedderSource), WebBuildError> {
    let app_dir = project_dir.join(web.host_dir_or_default());
    if EMBEDDER_FILES
        .iter()
        .all(|file| app_dir.join(file).is_file())
    {
        return Ok((app_dir, EmbedderSource::App));
    }
    locate_embedder(project_dir, locator).map(|dir| (dir, EmbedderSource::Framework))
}

/// The framework's host-page directory for this project — `platform/web`
/// inside the `frust-shell-web` package cargo resolves in its dependency
/// graph — verified to carry both [`EMBEDDER_FILES`].
///
/// Resolved by `cargo metadata` ([`crate::packages::locate`]) rather than
/// from a manifest key or the `frust` dependency's own value, because cargo
/// is what compiles the app: its answer is the shell crate actually built,
/// whether that is a checkout's or a downloaded release's.
///
/// Fails with a typed error rather than degrading: unlike a missing icon (a
/// quality question the desktop pipeline answers with a note), a missing
/// embedder means the artifact directory would contain a `pkg/` and no page
/// to load it from — servable, and broken in the browser. A build only needs
/// this when the project supplies no host page of its own.
pub fn embedder_dir(project_dir: &Path) -> Result<PathBuf, WebBuildError> {
    let dir = packages::locate(project_dir, EMBEDDER_PACKAGE)
        .map_err(|source| WebBuildError::EmbedderUnlocated { source })?;
    embedder_page(dir)
}

/// [`embedder_dir`] through an injected locator — the form the pipeline, the
/// preflight and `frust doctor` use, so one run asks cargo through the runner
/// it was given (and, behind a [`crate::packages::CachedLocator`], once).
pub fn embedder_dir_with(
    locator: &dyn PackageLocator,
    project_dir: &Path,
) -> Result<PathBuf, WebBuildError> {
    locate_embedder(project_dir, locator)
}

/// The crate-internal spelling of [`embedder_dir_with`].
pub(super) fn locate_embedder(
    project_dir: &Path,
    locator: &dyn PackageLocator,
) -> Result<PathBuf, WebBuildError> {
    let dir = locator
        .locate(project_dir, EMBEDDER_PACKAGE)
        .map_err(|source| WebBuildError::EmbedderUnlocated { source })?;
    embedder_page(dir)
}

/// `platform/web` under the located package directory, checked for both
/// [`EMBEDDER_FILES`].
fn embedder_page(package_dir: PathBuf) -> Result<PathBuf, WebBuildError> {
    let dir = package_dir.join(EMBEDDER_PACKAGE_REL);
    for file in EMBEDDER_FILES {
        if !dir.join(file).is_file() {
            return Err(WebBuildError::EmbedderIncomplete { dir, missing: file });
        }
    }
    Ok(dir)
}

/// The app crate's package name, as `Cargo.toml`'s `[package] name` spells it
/// — the stem `cargo build` names its `wasm32` artifact after, once cargo's
/// own `-` → `_` normalization is applied by [`super::wasm_file_stems`], and
/// the fallback [`super::build`] resolves `[web] out-name` and `[app] name`
/// against when the project carries no `frust.toml` at all.
///
/// A hard error rather than a soft fallback: `Cargo.toml` is the one input
/// this pipeline always requires (a `frust.toml` is not — see the module doc
/// on `super`), so an unreadable or nameless manifest leaves nothing to look
/// for on disk.
pub(super) fn package_name(project_dir: &Path) -> Result<String, WebBuildError> {
    let manifest = project_dir.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).map_err(|source| WebBuildError::Io {
        action: "reading the app manifest",
        path: manifest.clone(),
        source,
    })?;
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|err| WebBuildError::PackageName {
            manifest: manifest.clone(),
            reason: err.to_string(),
        })?;
    doc.get("package")
        .and_then(|package| package.as_table_like())
        .and_then(|package| package.get("name"))
        .and_then(|name| name.as_str())
        .map(str::to_string)
        .ok_or(WebBuildError::PackageName {
            manifest,
            reason: "no `[package] name` key".to_string(),
        })
}

/// The basename `X` a host page's `"./pkg/X.js"` `?module=` default names, or
/// `None` when `html` does not carry that literal shape.
///
/// Deliberately lenient rather than a full HTML/JS parse: both shipped pages
/// (`crates/frust-drive/templates/app/web.tmpl/index.html.tmpl`'s render and
/// `crates/frust-shell-web/platform/web/index.html`) write the default as a plain JS string literal
/// (`|| "./pkg/<name>.js";`) this substring search finds directly, and a
/// hand-authored page that restructures the script is a page
/// [`verify_host_page_module`] declines to second-guess rather than misreads.
///
/// The search requires the literal quote on both sides (`"./pkg/` …
/// `.js"`), not a bare `./pkg/` substring: both shipped pages also describe
/// the same shape in prose, backtick-quoted (`` `?module=./pkg/<name>.js` ``)
/// ahead of the real default in file order — a bare substring search would
/// find that placeholder text first and misread its literal `<name>` as the
/// page's actual default. Requiring the surrounding double quotes matches
/// only the real JS string literal, which neither backtick-quoted prose ever
/// carries.
pub(super) fn page_module_default(html: &str) -> Option<String> {
    const MARKER: &str = "\"./pkg/";
    const TERMINATOR: &str = ".js\"";
    let start = html.find(MARKER)? + MARKER.len();
    let rest = &html[start..];
    let end = rest.find(TERMINATOR)?;
    let name = &rest[..end];
    if name.is_empty() || name.contains(['"', '\'', '/', '\\', '\n', ' ', '<', '>']) {
        None
    } else {
        Some(name.to_string())
    }
}

/// Refuses the build when the app's own staged page's `?module=` default
/// disagrees with `out_name` — the `wasm-bindgen --out-name` this build
/// resolved. A user who overrides `[web] out-name` without also editing their
/// page would otherwise get a build that succeeds and a browser console that
/// 404s: this pipeline stages pages verbatim (see the module doc), so it
/// cannot fix the mismatch itself and refuses instead, before the compile
/// (the module doc's ordering rule).
///
/// Only ever called for [`EmbedderSource::App`]: the framework page's default
/// is fixed and this pipeline's own `out_name` choice for it
/// ([`super::BINDGEN_OUT_NAME`]) is defined to match it, so there is nothing
/// to verify there.
pub(super) fn verify_host_page_module(
    embedder: &Path,
    out_name: &str,
) -> Result<(), WebBuildError> {
    let path = embedder.join("index.html");
    let html = fs::read_to_string(&path).map_err(|source| WebBuildError::Io {
        action: "reading the host page to verify its `?module=` default",
        path: path.clone(),
        source,
    })?;
    match page_module_default(&html) {
        Some(expected) if expected != out_name => Err(WebBuildError::OutNameHostPageMismatch {
            path,
            page_module: expected,
            out_name: out_name.to_string(),
        }),
        _ => Ok(()),
    }
}

/// Whether `dir` is a strict descendant of `project_dir` — never
/// `project_dir` itself, never reached through a `..` (or a bare `.`
/// contributing nothing) component. Lexical, like the rest of this guard:
/// symlinks are not resolved (see [`prepare_dir`]).
fn is_strictly_contained(dir: &Path, project_dir: &Path) -> bool {
    let Ok(suffix) = dir.strip_prefix(project_dir) else {
        return false;
    };
    let mut has_real_component = false;
    for component in suffix.components() {
        match component {
            Component::CurDir => continue,
            Component::Normal(_) => has_real_component = true,
            _ => return false,
        }
    }
    has_real_component
}

/// `path` with its `.` components dropped and each `..` folded into the
/// component before it (a leading `..` with nothing to fold into is kept).
/// Purely lexical — no filesystem access, no symlink resolution — so the
/// overlap guards below can compare a path carrying `..` components (a
/// configured `[web] host-dir` or `out-dir` such as `../site`) against
/// another without a `..` in the middle hiding a real overlap.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let folded = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if folded {
                    out.pop();
                } else {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether `a` and `b` overlap on the filesystem: one is a descendant of
/// the other, or they are the same. Lexical, like the other guards, after
/// [`normalize_lexically`] on both sides.
fn paths_overlap(a: &Path, b: &Path) -> bool {
    let a = normalize_lexically(a);
    let b = normalize_lexically(b);
    a == b || b.strip_prefix(&a).is_ok() || a.strip_prefix(&b).is_ok()
}

/// Every host-page directory a browser build must never delete, whatever
/// page it resolves: the app's configured `<host-dir>` (present, partial, or
/// absent — a directory the developer named is protected before it is
/// populated), the framework's page when `locator` finds the
/// `frust-shell-web` package, and the page the build actually resolved
/// (`embedder`, when the caller has one). Deduplicated, in that order. The
/// project's `src/` is not listed because [`guard_artifact_dir`] protects it
/// unconditionally — no caller can leave it out.
///
/// The set is deliberately independent of [`resolve_embedder`] succeeding,
/// and that is the whole point: a partial app page (`index.html` without
/// `frust_web.js`) makes resolution fall through to the framework page, and a
/// guard that protected only the *resolved* page then let `[web] out-dir =
/// "web"` delete the half-written one. [`prepare_dir`] refuses an artifact
/// directory overlapping any of these, and the preflight's artifact-safety
/// row reports the same set, so the two never disagree about what is at risk.
pub(super) fn protected_dirs(
    project_dir: &Path,
    web: &WebSection,
    embedder: Option<&Path>,
    locator: &dyn PackageLocator,
) -> Vec<PathBuf> {
    let mut dirs = vec![project_dir.join(web.host_dir_or_default())];
    let mut protect = |dir: PathBuf| {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };
    if let Ok(framework) = locate_embedder(project_dir, locator) {
        protect(framework);
    }
    if let Some(embedder) = embedder {
        protect(embedder.to_path_buf());
    }
    dirs
}

/// The first of `protected` that `dir` overlaps ([`paths_overlap`]), if any.
fn artifact_dir_conflict<'a>(dir: &Path, protected: &'a [PathBuf]) -> Option<&'a Path> {
    protected
        .iter()
        .find(|candidate| paths_overlap(dir, candidate))
        .map(PathBuf::as_path)
}

/// The refusal half of [`prepare_dir`], with no side effect: `dir` must be a
/// strict descendant of `project_dir` ([`is_strictly_contained`], else
/// [`WebBuildError::UnsafeArtifactDir`]) and overlap neither any of
/// `protected` ([`protected_dirs`]) nor the project's `src/` — the latter
/// checked here unconditionally, so the source tree stays safe whatever
/// host-page set a caller assembles (else
/// [`WebBuildError::ArtifactDirOverlapsSource`], naming the directory it
/// would have destroyed). [`super::build`] runs this before the compile, so a
/// misconfigured `[web] out-dir` costs no build, and [`prepare_dir`] runs it
/// again at the delete itself.
pub(super) fn guard_artifact_dir(
    dir: &Path,
    project_dir: &Path,
    protected: &[PathBuf],
) -> Result<(), WebBuildError> {
    if !is_strictly_contained(dir, project_dir) {
        return Err(WebBuildError::UnsafeArtifactDir {
            path: dir.to_path_buf(),
            project_dir: project_dir.to_path_buf(),
        });
    }
    let src = [project_dir.join("src")];
    if let Some(clash) =
        artifact_dir_conflict(dir, protected).or_else(|| artifact_dir_conflict(dir, &src))
    {
        return Err(WebBuildError::ArtifactDirOverlapsSource {
            artifact_dir: dir.to_path_buf(),
            host_page_dir: clash.to_path_buf(),
        });
    }
    Ok(())
}

/// Creates `dir` fresh: an existing directory is removed first, so a rebuild
/// can never serve a previous run's file (a renamed module, a stale
/// `frust_web.js`) out of the artifact directory it hands back.
///
/// **Guarded**, for the reason [`crate::desktop_build::bundle`]'s twin is:
/// `remove_dir_all` is the most dangerous primitive in this module, so the
/// call is [`guard_artifact_dir`]'s typed refusal — touching nothing — unless
/// `dir` is a strict descendant of `project_dir` and overlaps neither `src/`
/// nor any of `protected` (see [`protected_dirs`]: the configured host
/// directory, the framework page, the resolved page). `[web] out-dir` is
/// project-configurable (unlike the old hard-coded `dist/web`), so the
/// containment boundary is the project directory itself rather than a fixed
/// subdirectory of it — an out-dir of `"."`, `".."`, or anything that steps
/// outside the project is refused the same way a `dist/web/../../..`
/// traversal always was.
pub(super) fn prepare_dir(
    dir: &Path,
    project_dir: &Path,
    protected: &[PathBuf],
) -> Result<(), WebBuildError> {
    guard_artifact_dir(dir, project_dir, protected)?;
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|source| WebBuildError::Io {
            action: "removing the previous artifact directory at",
            path: dir.to_path_buf(),
            source,
        })?;
    }
    create_dir(dir)
}

/// `mkdir -p`.
pub(super) fn create_dir(dir: &Path) -> Result<(), WebBuildError> {
    fs::create_dir_all(dir).map_err(|source| WebBuildError::Io {
        action: "creating directory",
        path: dir.to_path_buf(),
        source,
    })
}

/// Copies `from` to `to`, creating `to`'s parent directories.
pub(super) fn copy_file(from: &Path, to: &Path) -> Result<(), WebBuildError> {
    if let Some(parent) = to.parent() {
        create_dir(parent)?;
    }
    fs::copy(from, to).map_err(|source| WebBuildError::Io {
        action: "copying into the artifact directory",
        path: from.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// Stages the resolved embedder's host page over an already-populated
/// artifact directory, returning the files written in copy order.
///
/// Runs **after** `wasm-bindgen`, not before: `wasm-bindgen --out-dir` writes
/// into `pkg/` only, so the two never collide, but ordering the copy last
/// keeps "the directory is complete" and "the page is there" the same
/// observation for any caller that inspects a half-finished build.
pub(super) fn stage_embedder(
    embedder: &Path,
    artifact_dir: &Path,
) -> Result<Vec<PathBuf>, WebBuildError> {
    let mut written = Vec::with_capacity(EMBEDDER_FILES.len());
    for file in EMBEDDER_FILES {
        let to = artifact_dir.join(file);
        copy_file(&embedder.join(file), &to)?;
        written.push(to);
    }
    Ok(written)
}

/// Every file `wasm-bindgen` left in `pkg/`, sorted by file name so a report's
/// artifact list is stable across runs (directory iteration order is not).
///
/// Non-recursive by construction: `wasm-bindgen --target web` emits a flat
/// `pkg/` (the `.js` glue, the `_bg.wasm` module, and their `.d.ts`
/// companions), and a nested directory there would be something this pipeline
/// did not put there.
pub(super) fn pkg_artifacts(pkg_dir: &Path) -> Result<Vec<PathBuf>, WebBuildError> {
    let entries = fs::read_dir(pkg_dir).map_err(|source| WebBuildError::Io {
        action: "listing the wasm-bindgen output directory",
        path: pkg_dir.to_path_buf(),
        source,
    })?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    Ok(files)
}

/// The locator the web fake-checkout fixtures share (this module's
/// `checkout_with_app`, `web_build`'s and the preflight's `checkout`): a
/// project at `<checkout>/examples/<app>` finds `frust-shell-web` at
/// `<checkout>/crates/frust-shell-web` when that directory exists — what
/// cargo would answer for the fixtures' `frust = { path =
/// "../../crates/frust" }` — and no such package otherwise. A stub, so no
/// fixture test runs cargo against a checkout that holds no real crates.
#[cfg(test)]
pub(super) fn fixture_locator(project_dir: &Path) -> packages::StubLocator {
    let shell = project_dir
        .parent()
        .and_then(Path::parent)
        .map(|checkout| checkout.join("crates").join(EMBEDDER_PACKAGE));
    match shell {
        Some(dir) if dir.is_dir() => packages::StubLocator::new().with(EMBEDDER_PACKAGE, dir),
        _ => packages::StubLocator::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// [`super::locate_embedder`] through the fixtures' stubbed locator.
    fn embedder_dir(project_dir: &Path) -> Result<PathBuf, WebBuildError> {
        locate_embedder(project_dir, &fixture_locator(project_dir))
    }

    /// [`super::resolve_embedder`] through the fixtures' stubbed locator.
    fn resolve_embedder(
        project_dir: &Path,
        web: &WebSection,
    ) -> Result<(PathBuf, EmbedderSource), WebBuildError> {
        super::resolve_embedder(project_dir, web, &fixture_locator(project_dir))
    }

    /// [`super::protected_dirs`] through the fixtures' stubbed locator.
    fn protected_dirs(
        project_dir: &Path,
        web: &WebSection,
        embedder: Option<&Path>,
    ) -> Vec<PathBuf> {
        super::protected_dirs(project_dir, web, embedder, &fixture_locator(project_dir))
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-web-bundle-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Lays out a fake framework checkout with a
    /// `crates/frust-shell-web/platform/web` embedder and an app project whose
    /// `frust` dependency points into it, returning `(checkout, project)`.
    /// Mirrors the real relative shape (`<repo>/crates/frust`,
    /// `<repo>/crates/frust-shell-web`), which is what [`fixture_locator`]
    /// answers from.
    fn checkout_with_app(tag: &str, files: &[&str]) -> (PathBuf, PathBuf) {
        let root = temp_dir(tag);
        let embedder = root.join("crates/frust-shell-web/platform/web");
        fs::create_dir_all(&embedder).unwrap();
        for file in files {
            fs::write(embedder.join(file), "// stub\n").unwrap();
        }
        fs::create_dir_all(root.join("crates/frust")).unwrap();
        let project = root.join("examples/app");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"my-app\"\n\n[dependencies]\nfrust = { path = \"../../crates/frust\" }\n",
        )
        .unwrap();
        (root, project)
    }

    #[test]
    fn the_artifact_dir_honours_out_dir_and_defaults_to_build_web() {
        assert_eq!(
            artifact_dir(Path::new("/p"), &WebSection::default()),
            PathBuf::from("/p/build/web"),
            "the layout front-ends print and serve must not drift"
        );
        let overridden = WebSection {
            out_dir: Some("dist".to_string()),
            ..WebSection::default()
        };
        assert_eq!(
            artifact_dir(Path::new("/p"), &overridden),
            PathBuf::from("/p/dist")
        );
    }

    #[test]
    fn the_embedder_is_platform_web_inside_the_located_shell_package() {
        let (root, project) = checkout_with_app("embedder-ok", EMBEDDER_FILES);
        let dir = embedder_dir(&project).unwrap();
        assert!(dir.join("frust_web.js").is_file());
        // `platform/web` under the package directory the locator answered,
        // not somewhere that merely happens to contain the two files.
        assert_eq!(
            dir.canonicalize().unwrap(),
            root.join("crates/frust-shell-web/platform/web")
                .canonicalize()
                .unwrap()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_embedder_missing_the_glue_module_is_a_typed_refusal() {
        let (root, project) = checkout_with_app("embedder-partial", &["index.html"]);
        let err = embedder_dir(&project).unwrap_err();
        assert!(
            matches!(&err, WebBuildError::EmbedderIncomplete { missing, .. } if *missing == "frust_web.js"),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A graph with no `frust-shell-web` is a typed refusal carrying the
    /// locator's own reason, which names how the project depends on frust.
    #[test]
    fn an_unlocatable_shell_package_is_a_typed_refusal_naming_the_frust_dependency() {
        let dir = temp_dir("no-frust-dep");
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        let err = embedder_dir(&dir).unwrap_err();
        assert!(
            matches!(
                &err,
                WebBuildError::EmbedderUnlocated {
                    source: packages::PackagesError::PackageAbsent { .. }
                }
            ),
            "{err:?}"
        );
        let message = err.to_string();
        assert!(message.contains("frust-shell-web"), "{message}");
        assert!(message.contains("no `frust` entry"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A `cargo metadata` failure (offline, a moved checkout) reaches the
    /// caller with cargo's own words.
    #[test]
    fn a_failed_package_lookup_carries_cargos_message() {
        let dir = temp_dir("lookup-fails");
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        let locator =
            packages::StubLocator::failing("error: failed to load source for dependency `frust`");
        let err = locate_embedder(&dir, &locator).unwrap_err();
        assert!(
            err.to_string()
                .contains("failed to load source for dependency `frust`"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The precedence's first branch: an app that supplies its own complete
    /// host page never touches the framework, even one that cannot resolve.
    #[test]
    fn resolve_embedder_prefers_the_apps_own_host_page() {
        let project = temp_dir("resolve-app-page");
        fs::write(project.join("Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        let host = project.join("web");
        fs::create_dir_all(&host).unwrap();
        for file in EMBEDDER_FILES {
            fs::write(host.join(file), "// stub\n").unwrap();
        }
        let (dir, source) = resolve_embedder(&project, &WebSection::default()).unwrap();
        assert_eq!(dir, host);
        assert_eq!(source, EmbedderSource::App);
        let _ = fs::remove_dir_all(&project);
    }

    /// `[web] host-dir` is read, not just the default.
    #[test]
    fn resolve_embedder_honours_a_custom_host_dir() {
        let project = temp_dir("resolve-custom-host-dir");
        fs::write(project.join("Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        let host = project.join("page");
        fs::create_dir_all(&host).unwrap();
        for file in EMBEDDER_FILES {
            fs::write(host.join(file), "// stub\n").unwrap();
        }
        let web = WebSection {
            host_dir: Some("page".to_string()),
            ..WebSection::default()
        };
        let (dir, source) = resolve_embedder(&project, &web).unwrap();
        assert_eq!(dir, host);
        assert_eq!(source, EmbedderSource::App);
        let _ = fs::remove_dir_all(&project);
    }

    /// No app host page at all (`examples/web-gallery`'s shape: an
    /// `index.html` at the project root, not under `web/`) falls back to the
    /// framework embedder.
    #[test]
    fn resolve_embedder_falls_back_to_the_framework_when_no_app_page_exists() {
        let (root, project) = checkout_with_app("resolve-fallback", EMBEDDER_FILES);
        let (dir, source) = resolve_embedder(&project, &WebSection::default()).unwrap();
        assert_eq!(
            dir.canonicalize().unwrap(),
            root.join("crates/frust-shell-web/platform/web")
                .canonicalize()
                .unwrap()
        );
        assert_eq!(source, EmbedderSource::Framework);
        let _ = fs::remove_dir_all(&root);
    }

    /// An app page missing one of its two files is read as "no app page",
    /// not as a partial-page error — the framework is tried next.
    #[test]
    fn resolve_embedder_treats_a_partial_app_page_as_absent() {
        let (root, project) = checkout_with_app("resolve-partial-app-page", EMBEDDER_FILES);
        fs::create_dir_all(project.join("web")).unwrap();
        fs::write(project.join("web/index.html"), "<!doctype html>").unwrap();
        let (_, source) = resolve_embedder(&project, &WebSection::default()).unwrap();
        assert_eq!(source, EmbedderSource::Framework);
        let _ = fs::remove_dir_all(&root);
    }

    /// Neither page resolving surfaces the framework's own typed error.
    #[test]
    fn resolve_embedder_propagates_the_framework_error_when_neither_page_exists() {
        let dir = temp_dir("resolve-neither");
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        assert!(matches!(
            resolve_embedder(&dir, &WebSection::default()),
            Err(WebBuildError::EmbedderUnlocated { .. })
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_package_name_comes_from_cargo_toml() {
        let (root, project) = checkout_with_app("pkg-name", EMBEDDER_FILES);
        assert_eq!(package_name(&project).unwrap(), "my-app");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_nameless_manifest_is_a_typed_refusal() {
        let dir = temp_dir("nameless");
        fs::write(dir.join("Cargo.toml"), "[dependencies]\n").unwrap();
        assert!(matches!(
            package_name(&dir),
            Err(WebBuildError::PackageName { .. })
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn page_module_default_reads_the_scaffolded_and_framework_shapes() {
        let scaffolded = "const moduleUrl =\n  new URLSearchParams(location.search).get(\"module\") || \"./pkg/myapp.js\";";
        assert_eq!(page_module_default(scaffolded).as_deref(), Some("myapp"));
        let framework = "|| \"./pkg/app.js\";";
        assert_eq!(page_module_default(framework).as_deref(), Some("app"));
        assert_eq!(page_module_default("<!doctype html>no module here"), None);
    }

    /// Regression: both shipped pages describe the same `?module=` shape in
    /// backtick-quoted prose (`` `?module=./pkg/<name>.js` ``) BEFORE the real
    /// quoted default in file order. A bare `./pkg/` substring search finds
    /// that placeholder text first and misreads its literal `<name>` as the
    /// page's actual default, refusing every build of a freshly scaffolded
    /// app outright — this is the real shape `crates/frust-drive/templates/app/web.tmpl/`
    /// renders, reproduced verbatim.
    #[test]
    fn page_module_default_skips_backtick_quoted_prose_ahead_of_the_real_default() {
        let html = "\
             <!--\n\
             load. Defaults to `./pkg/w205app.js` so this page works unedited once a\n\
             with `?module=./pkg/<name>.js`, matching the `[web] out-name` your build\n\
             -->\n\
             <script type=\"module\">\n\
               // `?module=./pkg/<name>.js` selects which app's wasm-bindgen glue to\n\
               const moduleUrl =\n\
                 new URLSearchParams(location.search).get(\"module\") || \"./pkg/w205app.js\";\n\
             </script>\n";
        assert_eq!(page_module_default(html).as_deref(), Some("w205app"));
    }

    #[test]
    fn verify_host_page_module_passes_when_the_default_matches() {
        let project = temp_dir("verify-match");
        fs::create_dir_all(project.join("web")).unwrap();
        fs::write(project.join("web/index.html"), "|| \"./pkg/myapp.js\";").unwrap();
        verify_host_page_module(&project.join("web"), "myapp").unwrap();
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn verify_host_page_module_refuses_a_mismatch_naming_both_values() {
        let project = temp_dir("verify-mismatch");
        fs::create_dir_all(project.join("web")).unwrap();
        fs::write(project.join("web/index.html"), "|| \"./pkg/myapp.js\";").unwrap();
        let err = verify_host_page_module(&project.join("web"), "renamed").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("myapp"), "{message}");
        assert!(message.contains("renamed"), "{message}");
        assert!(matches!(err, WebBuildError::OutNameHostPageMismatch { .. }));
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn verify_host_page_module_is_lenient_about_an_unrecognized_page_shape() {
        let project = temp_dir("verify-lenient");
        fs::create_dir_all(project.join("web")).unwrap();
        fs::write(project.join("web/index.html"), "<!doctype html>\n").unwrap();
        verify_host_page_module(&project.join("web"), "whatever").unwrap();
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn preparing_the_artifact_dir_clears_a_previous_build() {
        let project = temp_dir("prepare");
        let dir = project.join("build/web");
        let embedder = project.join("web");
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::write(dir.join("pkg/stale.js"), "old").unwrap();
        prepare_dir(&dir, &project, std::slice::from_ref(&embedder)).unwrap();
        assert!(dir.is_dir());
        assert!(!dir.join("pkg/stale.js").exists());
        let _ = fs::remove_dir_all(&project);
    }

    /// The guard's whole point: every path that is not a strict descendant of
    /// `project_dir` is refused before anything is removed — including an
    /// `out-dir` of `"."` or `".."`, which a hard-coded `dist/` root never
    /// had to consider.
    #[test]
    fn preparing_a_directory_outside_the_project_is_refused() {
        let project = temp_dir("prepare-guard");
        let embedder = project.join("web");
        for candidate in [
            project.clone(),
            project.join("."),
            project.join("build/web/../../.."),
            project.join(".."),
            PathBuf::from("/"),
        ] {
            let err =
                prepare_dir(&candidate, &project, std::slice::from_ref(&embedder)).unwrap_err();
            assert!(
                matches!(err, WebBuildError::UnsafeArtifactDir { .. }),
                "{candidate:?} was not refused: {err:?}"
            );
        }
        assert!(project.is_dir(), "nothing may be removed by a refusal");
        let _ = fs::remove_dir_all(&project);
    }

    /// A nested or single-segment `out-dir` under the project is allowed —
    /// only escaping or resolving to the project root itself is refused.
    #[test]
    fn preparing_any_strict_descendant_of_the_project_is_allowed() {
        let project = temp_dir("prepare-allowed");
        let embedder = project.join("web");
        for candidate in ["dist", "build/web", "output/site"] {
            let dir = project.join(candidate);
            prepare_dir(&dir, &project, std::slice::from_ref(&embedder)).unwrap();
            assert!(dir.is_dir(), "{candidate}");
        }
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn staging_copies_both_embedder_files_verbatim() {
        let (root, project) = checkout_with_app("stage", EMBEDDER_FILES);
        let embedder = embedder_dir(&project).unwrap();
        fs::write(embedder.join("index.html"), "<!doctype html>\n").unwrap();
        let dir = artifact_dir(&project, &WebSection::default());
        prepare_dir(&dir, &project, std::slice::from_ref(&embedder)).unwrap();
        let written = stage_embedder(&embedder, &dir).unwrap();
        assert_eq!(
            written,
            vec![dir.join("index.html"), dir.join("frust_web.js")]
        );
        assert_eq!(
            fs::read_to_string(dir.join("index.html")).unwrap(),
            "<!doctype html>\n",
            "the host page is staged byte-for-byte, never rewritten"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn pkg_artifacts_lists_files_in_a_stable_order() {
        let dir = temp_dir("pkg-list");
        fs::create_dir_all(dir.join("nested")).unwrap();
        for name in ["app.js", "app_bg.wasm", "app.d.ts"] {
            fs::write(dir.join(name), "x").unwrap();
        }
        let files: Vec<String> = pkg_artifacts(&dir)
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(files, vec!["app.d.ts", "app.js", "app_bg.wasm"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Artifact dir equals the app's own host page — a deletion would destroy
    /// the checked-in host page source.
    #[test]
    fn preparing_artifact_dir_equal_to_app_embedder_is_refused() {
        let project = temp_dir("overlap-equal-app");
        let embedder = project.join("web");
        fs::create_dir_all(&embedder).unwrap();
        let err = prepare_dir(&embedder, &project, std::slice::from_ref(&embedder)).unwrap_err();
        assert!(
            matches!(err, WebBuildError::ArtifactDirOverlapsSource { .. }),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&project);
    }

    /// Artifact dir is the parent of the app's own host page — a deletion would
    /// destroy the checked-in host page source.
    #[test]
    fn preparing_artifact_dir_containing_app_embedder_is_refused() {
        let project = temp_dir("overlap-contain-app");
        let artifact = project.join("build");
        let embedder = artifact.join("web");
        fs::create_dir_all(&embedder).unwrap();
        let err = prepare_dir(&artifact, &project, std::slice::from_ref(&embedder)).unwrap_err();
        assert!(
            matches!(err, WebBuildError::ArtifactDirOverlapsSource { .. }),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&project);
    }

    /// Artifact dir is inside the app's own host page — a deletion would destroy
    /// the checked-in host page source.
    #[test]
    fn preparing_artifact_dir_inside_app_embedder_is_refused() {
        let project = temp_dir("overlap-contained-app");
        let embedder = project.join("web");
        fs::create_dir_all(&embedder).unwrap();
        let artifact = embedder.join("assets");
        let err = prepare_dir(&artifact, &project, std::slice::from_ref(&embedder)).unwrap_err();
        assert!(
            matches!(err, WebBuildError::ArtifactDirOverlapsSource { .. }),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&project);
    }

    /// Artifact dir equals the framework's host page — a deletion would destroy
    /// the framework source.
    #[test]
    fn preparing_artifact_dir_equal_to_framework_embedder_is_refused() {
        let project = temp_dir("overlap-equal-fw");
        // Create a mock framework-like structure
        let embedder = project.join("framework/crates/frust-shell-web/platform/web");
        fs::create_dir_all(&embedder).unwrap();
        let err = prepare_dir(&embedder, &project, std::slice::from_ref(&embedder)).unwrap_err();
        assert!(
            matches!(err, WebBuildError::ArtifactDirOverlapsSource { .. }),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&project);
    }

    /// Artifact dir equals the project's src directory — a deletion would
    /// destroy the source code.
    #[test]
    fn preparing_artifact_dir_equal_to_src_is_refused() {
        let project = temp_dir("overlap-equal-src");
        let src = project.join("src");
        fs::create_dir_all(&src).unwrap();
        let embedder = project.join("web");
        let err = prepare_dir(&src, &project, std::slice::from_ref(&embedder)).unwrap_err();
        assert!(
            matches!(err, WebBuildError::ArtifactDirOverlapsSource { .. }),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&project);
    }

    /// Artifact dir is inside the src directory — a deletion would destroy
    /// the source code.
    #[test]
    fn preparing_artifact_dir_inside_src_is_refused() {
        let project = temp_dir("overlap-contained-src");
        let src = project.join("src");
        let artifact = src.join("build");
        fs::create_dir_all(&src).unwrap();
        let embedder = project.join("web");
        let err = prepare_dir(&artifact, &project, std::slice::from_ref(&embedder)).unwrap_err();
        assert!(
            matches!(err, WebBuildError::ArtifactDirOverlapsSource { .. }),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&project);
    }

    /// Artifact dir as a sibling of the embedder and src is allowed — no overlap.
    #[test]
    fn preparing_artifact_dir_separate_from_embedder_and_src_is_allowed() {
        let project = temp_dir("overlap-sibling");
        let embedder = project.join("web");
        let src = project.join("src");
        let artifact = project.join("build/web");
        fs::create_dir_all(&embedder).unwrap();
        fs::create_dir_all(&src).unwrap();
        prepare_dir(&artifact, &project, std::slice::from_ref(&embedder)).unwrap();
        assert!(artifact.is_dir());
        let _ = fs::remove_dir_all(&project);
    }

    /// The configured host directory is protected even when the page in it is
    /// incomplete: `resolve_embedder` reads `web/index.html` without
    /// `web/frust_web.js` as "no app page" and falls through to the framework
    /// page, but `[web] out-dir = "web"` would still have deleted that
    /// `index.html` under a guard that protected only the resolved page.
    #[test]
    fn a_partial_app_page_still_protects_the_configured_host_dir() {
        let (root, project) = checkout_with_app("partial-host-dir", EMBEDDER_FILES);
        let host = project.join("web");
        fs::create_dir_all(&host).unwrap();
        fs::write(host.join("index.html"), "<!doctype html>").unwrap();
        let web = WebSection {
            out_dir: Some("web".to_string()),
            ..WebSection::default()
        };
        let (embedder, source) = resolve_embedder(&project, &web).unwrap();
        assert_eq!(
            source,
            EmbedderSource::Framework,
            "precondition: the partial page falls through to the framework page"
        );
        let protected = protected_dirs(&project, &web, Some(&embedder));
        let err = prepare_dir(&artifact_dir(&project, &web), &project, &protected).unwrap_err();
        assert!(
            matches!(
                &err,
                WebBuildError::ArtifactDirOverlapsSource { host_page_dir, .. } if *host_page_dir == host
            ),
            "{err:?}"
        );
        assert!(
            host.join("index.html").is_file(),
            "a refusal removes nothing"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// `protected_dirs` names every host-page directory at risk — the
    /// configured host dir whether or not it exists, the framework page when
    /// locatable, the resolved page — deduplicated, and still yields the
    /// configured dir when nothing resolves at all. `src/` is the guard's
    /// own unconditional business: refused even against an empty set.
    #[test]
    fn protected_dirs_cover_the_host_dir_and_the_framework_page() {
        let (root, project) = checkout_with_app("protected", EMBEDDER_FILES);
        let framework = embedder_dir(&project).unwrap();
        let web = WebSection {
            host_dir: Some("page".to_string()),
            ..WebSection::default()
        };
        assert_eq!(
            protected_dirs(&project, &web, Some(&framework)),
            vec![project.join("page"), framework.clone()],
            "the resolved framework page is not listed twice"
        );
        let app_page = project.join("page");
        assert_eq!(
            protected_dirs(&project, &web, Some(&app_page)),
            vec![app_page, framework]
        );
        let bare = temp_dir("protected-bare");
        fs::write(bare.join("Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        assert_eq!(
            protected_dirs(&bare, &WebSection::default(), None),
            vec![bare.join("web")]
        );
        let err = guard_artifact_dir(&bare.join("src/out"), &bare, &[]).unwrap_err();
        assert!(
            matches!(
                &err,
                WebBuildError::ArtifactDirOverlapsSource { host_page_dir, .. } if *host_page_dir == bare.join("src")
            ),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&bare);
    }

    /// Overlap detection sees through `..` components, so a configured path
    /// that climbs out and back in (or a vendored framework checkout inside
    /// the project reached that way) is still recognized.
    #[test]
    fn overlap_detection_folds_parent_components() {
        let project = Path::new("/p");
        let framework = project.join("vendor/frust/crates/frust/../frust-shell-web/platform/web");
        assert!(paths_overlap(
            &project.join("vendor/frust/crates/frust-shell-web/platform/web/out"),
            &framework
        ));
        assert!(paths_overlap(&project.join("vendor/frust"), &framework));
        assert!(!paths_overlap(&project.join("build/web"), &framework));
        assert_eq!(
            normalize_lexically(&project.join("a/./b/../c")),
            project.join("a/c")
        );
        assert_eq!(
            normalize_lexically(Path::new("../x/./y")),
            PathBuf::from("../x/y"),
            "a leading `..` with nothing to fold into is kept"
        );
    }
}
