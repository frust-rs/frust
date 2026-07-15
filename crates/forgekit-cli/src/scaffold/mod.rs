//! Data-driven project scaffolding (spec §12.3), modeled on Flutter's
//! `templates/` mechanism: a manifest-listed file tree rendered against a
//! [`TemplateContext`].

pub mod context;
pub mod renderer;

#[allow(unused_imports)] // NameError: public API surface for future callers matching on variants
pub use context::{title_case, validate_project_name, NameError, TemplateContext};

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use include_dir::{include_dir, Dir};

/// The `templates/app/` tree, embedded into the `forgekit` binary at
/// compile time so `forgekit create` works standalone without a repo
/// checkout at runtime.
static EMBEDDED_APP_TEMPLATE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../templates/app");

/// Name of the manifest file (relative to the template root) that
/// whitelists every file the template ships. Never itself copied into a
/// generated project.
const MANIFEST_FILE: &str = "template_manifest.json";

/// How a manifest entry's content should be handled (spec §12.3 extension
/// conventions).
enum FileMode {
    /// `<file>.tmpl` → minijinja-render, strip `.tmpl`.
    Render,
    /// `<file>.copy.tmpl` → copy verbatim, strip `.copy.tmpl`.
    CopyVerbatim,
    /// Anything else in the manifest → copy as-is.
    CopyAsIs,
}

/// Splits a manifest entry into its content-handling mode and the logical
/// (post-suffix-stripping) relative path.
fn classify(entry: &str) -> (FileMode, &str) {
    if let Some(stem) = entry.strip_suffix(".copy.tmpl") {
        (FileMode::CopyVerbatim, stem)
    } else if let Some(stem) = entry.strip_suffix(".tmpl") {
        (FileMode::Render, stem)
    } else {
        (FileMode::CopyAsIs, entry)
    }
}

/// Where to read the template tree from: the binary's embedded copy, or a
/// filesystem override (`--template-dir`, undocumented, development only).
enum Source<'a> {
    Embedded,
    Dir(&'a Path),
}

impl Source<'_> {
    fn manifest(&self) -> Result<Vec<String>> {
        let raw = self.read_to_string(MANIFEST_FILE)?;
        serde_json::from_str(&raw).context("parsing template_manifest.json")
    }

    fn read(&self, relative: &str) -> Result<Vec<u8>> {
        match self {
            Source::Embedded => EMBEDDED_APP_TEMPLATE
                .get_file(relative)
                .map(|f| f.contents().to_vec())
                .ok_or_else(|| {
                    anyhow!("template file `{relative}` missing from embedded template (manifest drift)")
                }),
            Source::Dir(root) => fs::read(root.join(relative))
                .with_context(|| format!("reading template file `{relative}`")),
        }
    }

    fn read_to_string(&self, relative: &str) -> Result<String> {
        let bytes = self.read(relative)?;
        String::from_utf8(bytes)
            .with_context(|| format!("template file `{relative}` is not valid UTF-8"))
    }
}

/// Generates a new app at `dest` from the embedded `templates/app` tree (or
/// `template_dir_override`, for development). Returns the relative paths
/// written, in manifest order. Refuses a non-empty `dest` unless
/// `overwrite` is set.
pub fn generate(
    dest: &Path,
    ctx: &TemplateContext,
    template_dir_override: Option<&Path>,
    overwrite: bool,
) -> Result<Vec<PathBuf>> {
    check_destination(dest, overwrite)?;

    let source = match template_dir_override {
        Some(dir) => Source::Dir(dir),
        None => Source::Embedded,
    };
    let manifest = source.manifest()?;
    let render_vars = ctx.render_vars();
    let path_vars = ctx.path_vars();

    let mut written = Vec::with_capacity(manifest.len());
    for entry in &manifest {
        let (mode, logical) = classify(entry);
        let out_relative = renderer::expand_path(Path::new(logical), &path_vars);
        let out_path = dest.join(&out_relative);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating directory `{}`", parent.display()))?;
        }
        match mode {
            FileMode::Render => {
                let raw = source.read_to_string(entry)?;
                let rendered = renderer::render(&raw, &render_vars)
                    .with_context(|| format!("rendering `{entry}`"))?;
                fs::write(&out_path, rendered)
                    .with_context(|| format!("writing `{}`", out_path.display()))?;
            }
            FileMode::CopyVerbatim | FileMode::CopyAsIs => {
                let bytes = source.read(entry)?;
                fs::write(&out_path, bytes)
                    .with_context(|| format!("writing `{}`", out_path.display()))?;
            }
        }
        written.push(out_relative);
    }
    Ok(written)
}

/// Refuses a non-empty `dest` unless `overwrite` is set, listing the
/// offending files in the error.
fn check_destination(dest: &Path, overwrite: bool) -> Result<()> {
    if overwrite || !dest.exists() {
        return Ok(());
    }
    let entries: Vec<String> = fs::read_dir(dest)
        .with_context(|| format!("reading `{}`", dest.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    if entries.is_empty() {
        return Ok(());
    }
    bail!(
        "target directory `{}` is not empty (pass --overwrite to proceed):\n  {}",
        dest.display(),
        entries.join("\n  ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("forgekit-cli-test-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn test_context() -> TemplateContext {
        TemplateContext {
            project_name: "my_app".into(),
            title_case_name: "My App".into(),
            org: "dev.f0x".into(),
            description: "A new ForgeKit application.".into(),
            forgekit_version: "0.1.0".into(),
            forgekit_path: "/path/to/forgekit".into(),
        }
    }

    #[test]
    fn generate_produces_manifest_listed_files_with_substitutions() {
        let dest = unique_temp_dir("manifest-set");
        let ctx = test_context();

        let written = generate(&dest, &ctx, None, false).unwrap();
        assert!(!written.is_empty());

        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(cargo_toml.contains("my_app"), "{cargo_toml}");
        let forgekit_toml = fs::read_to_string(dest.join("forgekit.toml")).unwrap();
        assert!(forgekit_toml.contains("dev.f0x"), "{forgekit_toml}");

        assert!(dest.join("src/lib.rs").exists());
        assert!(dest.join("src/main.rs").exists());
        assert!(dest.join(".gitignore").exists());
        assert!(dest.join("assets/.gitkeep").exists());
        assert!(!dest.join("template_manifest.json").exists());
        assert!(!dest.join("Cargo.toml.tmpl").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_refuses_non_empty_dest_without_overwrite() {
        let dest = unique_temp_dir("refuse-non-empty");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("existing.txt"), b"x").unwrap();

        let ctx = test_context();
        let err = generate(&dest, &ctx, None, false).unwrap_err();
        assert!(err.to_string().contains("existing.txt"), "{err}");

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_allows_non_empty_dest_with_overwrite() {
        let dest = unique_temp_dir("allow-overwrite");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("existing.txt"), b"x").unwrap();

        let ctx = test_context();
        assert!(generate(&dest, &ctx, None, true).is_ok());
        assert!(dest.join("existing.txt").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_allows_empty_existing_dest() {
        let dest = unique_temp_dir("empty-existing");
        fs::create_dir_all(&dest).unwrap();

        let ctx = test_context();
        assert!(generate(&dest, &ctx, None, false).is_ok());

        let _ = fs::remove_dir_all(&dest);
    }
}
