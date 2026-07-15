//! `forgekit create` — data-driven project scaffolding (spec §12.3).

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};

use crate::scaffold::{self, TemplateContext};

/// Parsed + defaulted arguments for `forgekit create` (mirrors
/// `cli::Command::Create`; kept separate so `scaffold` stays decoupled
/// from `clap`).
pub struct CreateArgs {
    pub dir: String,
    pub org: String,
    pub project_name: Option<String>,
    pub description: String,
    pub overwrite: bool,
    /// `--template-dir` (undocumented, development only).
    pub template_dir: Option<String>,
    /// `--forgekit-path` (undocumented, development only).
    pub forgekit_path: Option<String>,
}

pub fn run(args: CreateArgs) -> Result<u8> {
    let dest = PathBuf::from(&args.dir);
    let project_name = match args.project_name {
        Some(name) => name,
        None => infer_project_name(&dest).with_context(|| {
            format!(
                "could not derive a project name from `{}`; pass --project-name",
                args.dir
            )
        })?,
    };
    scaffold::validate_project_name(&project_name)?;

    let ctx = TemplateContext {
        title_case_name: scaffold::title_case(&project_name),
        project_name: project_name.clone(),
        org: args.org,
        description: args.description,
        forgekit_version: env!("CARGO_PKG_VERSION").to_string(),
        forgekit_path: resolve_forgekit_path(args.forgekit_path.as_deref()),
    };

    let template_dir_override = args.template_dir.as_deref().map(Path::new);
    let written = scaffold::generate(&dest, &ctx, template_dir_override, args.overwrite)?;

    println!("Created {} file(s) in {}", written.len(), dest.display());
    println!();
    println!("All done! `{project_name}` is ready.");
    println!();
    println!("To run your app:");
    if args.dir != "." {
        println!("  cd {}", args.dir);
    }
    println!("  cargo run");

    Ok(0)
}

/// Resolves `--forgekit-path`, defaulting to this repo's `crates/forgekit`
/// (spec §12.3: a temporary mechanism until ForgeKit crates are
/// published), derived from `forgekit-cli`'s own compile-time manifest
/// directory.
fn resolve_forgekit_path(overridden: Option<&str>) -> String {
    if let Some(path) = overridden {
        return path.to_string();
    }
    let raw = Path::new(env!("CARGO_MANIFEST_DIR")).join("../forgekit");
    raw.canonicalize().unwrap_or(raw).to_string_lossy().into_owned()
}

/// Infers a project name from `dir`'s basename, resolving `.`/`..`
/// lexically — the target need not exist yet, so this can't use
/// `fs::canonicalize`.
fn infer_project_name(dir: &Path) -> Result<String> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let absolute = if dir.is_absolute() { dir.to_path_buf() } else { cwd.join(dir) };
    let normalized = normalize_lexically(&absolute);
    normalized
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .with_context(|| format!("`{}` has no basename to use as a project name", dir.display()))
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_project_name_uses_dir_basename() {
        assert_eq!(infer_project_name(Path::new("/tmp/my_app")).unwrap(), "my_app");
    }

    #[test]
    fn infer_project_name_resolves_dot_to_cwd_basename() {
        let cwd = std::env::current_dir().unwrap();
        let expected = cwd.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(infer_project_name(Path::new(".")).unwrap(), expected);
    }

    #[test]
    fn infer_project_name_resolves_trailing_parent_dir() {
        assert_eq!(infer_project_name(Path::new("/tmp/foo/../my_app")).unwrap(), "my_app");
    }
}
