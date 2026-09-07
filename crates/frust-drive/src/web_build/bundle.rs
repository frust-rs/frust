//! Assembly primitives for the browser artifact directory: where it lives,
//! the guarded rebuild of it, and the staging of `platform/web`'s host page
//! over a `wasm-bindgen` build's own output.
//!
//! The web tier's counterpart to [`crate::desktop_build::bundle`], and
//! deliberately much smaller: a browser "bundle" is a directory a static file
//! server can hand out, with no per-OS layout, no icon containers and no
//! signature. What it *does* share with the desktop layouts is the dangerous
//! primitive — a rebuild removes the previous artifact directory recursively —
//! so [`prepare_dir`] carries the same containment guard, positioned at the
//! delete itself rather than at the caller.
//!
//! # The embedder is consumed by path, never copied into the project
//!
//! `platform/web` is the browser embedder (`index.html` + `frust_web.js`), the
//! third host tier alongside `platform/android`'s `frust-embedding` and
//! `platform/ios`'s `FrustEmbedding`. Like those two it lives in the framework
//! checkout and is reached from a consuming app by path, derived the one way
//! this crate already derives every framework-relative directory:
//! `<frust>/../../<repo-relative path>`, where `<frust>` is the app's own
//! `frust` dependency path (the facade crate directory, two levels below the
//! repo root). `crate::plugin::apply`'s `resolve_sibling`/`plugin_dep_path`
//! and `crate::scaffold::context`'s `frust_embedding_android_dir` are the same
//! walk; those helpers are private to their own modules, so [`embedder_dir`]
//! restates it rather than widening someone else's surface — the convention is
//! shared, the code is not.
//!
//! This inherits the same machine-specific-checkout trade-off those accessors
//! document: an app whose `frust` path dependency has moved cannot build for
//! the browser until it is repointed. The mitigation is the same one, too —
//! the path is a placeholder for a published artifact once the embedder ships
//! to a registry.
//!
//! # Staged, not templated
//!
//! Both embedder files are copied **verbatim**. `index.html` resolves the app
//! module from `?module=`, defaulting to `./pkg/app.js`, so nothing in it
//! needs rewriting for an ordinary app — the pipeline instead names its
//! `wasm-bindgen` output `app` ([`super::BINDGEN_OUT_NAME`]) so a plain load
//! with no query string finds it. Choosing the output name over an edit keeps
//! a file this crate does not own byte-identical in the artifact directory,
//! which is what makes a hand edit to `platform/web/index.html` reach every
//! served build without a code change here.

use std::fs;
use std::path::{Component, Path, PathBuf};

use super::WebBuildError;

/// The project-relative output root every build target writes under — the same
/// `dist/` the desktop pipeline uses, so one directory holds every artifact a
/// project produces.
pub(super) const DIST_DIR: &str = "dist";

/// The `dist/` subdirectory this pipeline owns: `dist/web`.
pub(super) const WEB_DIR: &str = "web";

/// The artifact-directory subdirectory `wasm-bindgen` output lands in, fixed
/// by `platform/web/index.html`'s own `./pkg/<name>.js` default.
pub(super) const PKG_DIR: &str = "pkg";

/// The embedder's repo-root-relative directory.
const EMBEDDER_REL_PATH: &str = "platform/web";

/// The embedder files staged into every artifact directory, in copy order.
/// Both are required: `index.html` imports `./frust_web.js` directly, so a
/// directory carrying only the first serves a page that fails to boot.
pub(super) const EMBEDDER_FILES: &[&str] = &["index.html", "frust_web.js"];

/// Where a browser build for `project_dir` lands: `<project>/dist/web`.
///
/// Pure path arithmetic — nothing is created or checked. Public so a
/// front-end can name the directory (a "serving …" line, a `--open` URL)
/// without re-deriving the layout, and so [`serve`](super::serve) has an
/// obvious default root to be pointed at.
pub fn artifact_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(DIST_DIR).join(WEB_DIR)
}

/// The absolute `platform/web` directory this project's `frust` dependency
/// resolves to, verified to carry both [`EMBEDDER_FILES`].
///
/// Read from the project's own `Cargo.toml` rather than from a manifest key,
/// because the `frust` path dependency is the only value that already names
/// the framework checkout an app is built against — a second, hand-maintained
/// pointer would be free to disagree with the one cargo actually compiles.
///
/// Fails with a typed error rather than degrading: unlike a missing icon (a
/// quality question the desktop pipeline answers with a note), a missing
/// embedder means the artifact directory would contain a `pkg/` and no page
/// to load it from — servable, and broken in the browser.
pub fn embedder_dir(project_dir: &Path) -> Result<PathBuf, WebBuildError> {
    let manifest = project_dir.join("Cargo.toml");
    let frust_path = frust_dep_path(&manifest).ok_or_else(|| WebBuildError::NoFrustDependency {
        manifest: manifest.clone(),
    })?;
    let frust_abs = if Path::new(&frust_path).is_absolute() {
        PathBuf::from(&frust_path)
    } else {
        project_dir.join(&frust_path)
    };
    let dir = frust_abs.join("..").join("..").join(EMBEDDER_REL_PATH);
    for file in EMBEDDER_FILES {
        if !dir.join(file).is_file() {
            return Err(WebBuildError::EmbedderIncomplete {
                dir: dir.clone(),
                missing: file,
                frust_path: frust_path.clone(),
            });
        }
    }
    Ok(dir)
}

/// The `frust` dependency's `path` value, exactly as written (relative or
/// absolute). Mirrors `crate::plugin::apply`'s `frust_dep_path` — a
/// format-preserving read of one key, not a deserialize of the whole manifest,
/// so an app manifest carrying keys this crate has no type for is still
/// readable.
fn frust_dep_path(manifest: &Path) -> Option<String> {
    let text = fs::read_to_string(manifest).ok()?;
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
    let path = doc
        .get("dependencies")?
        .as_table_like()?
        .get("frust")?
        .as_table_like()?
        .get("path")?
        .as_str()?;
    Some(path.to_string())
}

/// The app crate's package name, as `Cargo.toml`'s `[package] name` spells it
/// — the stem `cargo build` names its `wasm32` artifact after, once cargo's
/// own `-` → `_` normalization is applied by [`super::wasm_file_stem`].
///
/// A hard error rather than a soft fallback (the desktop pipeline's stance for
/// the same value): there is no `frust.toml` display name to fall back to here
/// because this pipeline deliberately requires no `frust.toml` at all — a
/// plain `wasm32` app crate is a legitimate input — so an unreadable or
/// nameless manifest leaves nothing to look for on disk.
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

/// Creates `dir` fresh: an existing directory is removed first, so a rebuild
/// can never serve a previous run's file (a renamed module, a stale
/// `frust_web.js`) out of the artifact directory it hands back.
///
/// **Guarded**, for the reason [`crate::desktop_build::bundle`]'s twin is:
/// `remove_dir_all` is the most dangerous primitive in this module, so `dir`
/// must be a strict descendant of `<project_dir>/dist` or the call is a typed
/// refusal ([`WebBuildError::UnsafeArtifactDir`]) that touches nothing. The
/// check is lexical and runs in two parts, both needed — a `..` component
/// anywhere is refused outright (`Path::starts_with` compares components, so
/// `dist/web/../../..` "starts with" `dist` while resolving nowhere near it),
/// and what remains must sit under the dist root. Symlinks are not resolved:
/// [`fs::remove_dir_all`] unlinks a symlinked directory rather than following
/// it, so a link inside `dist/` cannot be used to delete the tree it points
/// at.
pub(super) fn prepare_dir(dir: &Path, project_dir: &Path) -> Result<(), WebBuildError> {
    let dist_root = project_dir.join(DIST_DIR);
    let contained = !dir.components().any(|c| matches!(c, Component::ParentDir))
        && dir.starts_with(&dist_root)
        && dir != dist_root;
    if !contained {
        return Err(WebBuildError::UnsafeArtifactDir {
            path: dir.to_path_buf(),
            dist_root,
        });
    }
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

/// Stages the embedder's host page over an already-populated artifact
/// directory, returning the files written in copy order.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

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

    /// Lays out a fake framework checkout with a `platform/web` embedder and an
    /// app project whose `frust` dependency points into it, returning the
    /// project directory. Mirrors the real relative shape exactly
    /// (`<repo>/crates/frust`, `<repo>/platform/web`) so the `../../` walk is
    /// exercised rather than asserted.
    fn checkout_with_app(tag: &str, files: &[&str]) -> (PathBuf, PathBuf) {
        let root = temp_dir(tag);
        let embedder = root.join("platform/web");
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
    fn the_artifact_dir_is_dist_web_under_the_project() {
        assert_eq!(
            artifact_dir(Path::new("/p")),
            PathBuf::from("/p/dist/web"),
            "the layout front-ends print and serve must not drift"
        );
    }

    #[test]
    fn the_embedder_resolves_through_the_frust_path_dependency() {
        let (root, project) = checkout_with_app("embedder-ok", EMBEDDER_FILES);
        let dir = embedder_dir(&project).unwrap();
        assert!(dir.join("frust_web.js").is_file());
        // The walk lands on the checkout's own `platform/web`, not somewhere
        // that merely happens to contain the two files.
        assert_eq!(
            dir.canonicalize().unwrap(),
            root.join("platform/web").canonicalize().unwrap()
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

    #[test]
    fn an_app_with_no_frust_path_dependency_names_its_manifest() {
        let dir = temp_dir("no-frust-dep");
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        assert!(matches!(
            embedder_dir(&dir),
            Err(WebBuildError::NoFrustDependency { .. })
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
    fn preparing_the_artifact_dir_clears_a_previous_build() {
        let project = temp_dir("prepare");
        let dir = artifact_dir(&project);
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::write(dir.join("pkg/stale.js"), "old").unwrap();
        prepare_dir(&dir, &project).unwrap();
        assert!(dir.is_dir());
        assert!(!dir.join("pkg/stale.js").exists());
        let _ = fs::remove_dir_all(&project);
    }

    /// The guard's whole point: every path that is not a strict descendant of
    /// `<project>/dist` is refused before anything is removed.
    #[test]
    fn preparing_a_directory_outside_dist_is_refused() {
        let project = temp_dir("prepare-guard");
        for candidate in [
            project.clone(),
            project.join("src"),
            project.join(DIST_DIR),
            project.join("dist/web/../../.."),
            PathBuf::from("/"),
        ] {
            let err = prepare_dir(&candidate, &project).unwrap_err();
            assert!(
                matches!(err, WebBuildError::UnsafeArtifactDir { .. }),
                "{candidate:?} was not refused: {err:?}"
            );
        }
        assert!(project.is_dir(), "nothing may be removed by a refusal");
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn staging_copies_both_embedder_files_verbatim() {
        let (root, project) = checkout_with_app("stage", EMBEDDER_FILES);
        let embedder = embedder_dir(&project).unwrap();
        fs::write(embedder.join("index.html"), "<!doctype html>\n").unwrap();
        let dir = artifact_dir(&project);
        prepare_dir(&dir, &project).unwrap();
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
}
