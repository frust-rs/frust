//! `frust create` — data-driven project scaffolding (spec §12.3).

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};

use crate::scaffold::{self, TemplateContext};

/// Parsed + defaulted arguments for `frust create` (mirrors
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
    /// `--frust-path` (undocumented, development only).
    pub frust_path: Option<String>,
    /// `--deeplink-scheme` (task 07).
    pub deeplink_scheme: Option<String>,
    /// `--deeplink-host` (task 07).
    pub deeplink_host: Option<String>,
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
    if let Some(scheme) = args.deeplink_scheme.as_deref() {
        scaffold::validate_deeplink_scheme(scheme)
            .with_context(|| format!("invalid --deeplink-scheme `{scheme}`"))?;
    }

    let ctx = TemplateContext {
        title_case_name: scaffold::title_case(&project_name),
        project_name: project_name.clone(),
        org: args.org,
        description: args.description,
        frust_version: env!("CARGO_PKG_VERSION").to_string(),
        frust_path: resolve_frust_path(args.frust_path.as_deref()),
        deeplink_scheme: args.deeplink_scheme,
        deeplink_host: args.deeplink_host,
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

/// Resolves `--frust-path`, defaulting to this repo's `crates/frust`
/// (spec §12.3: a temporary mechanism until Frust crates are
/// published), derived from `frust-cli`'s own compile-time manifest
/// directory.
fn resolve_frust_path(overridden: Option<&str>) -> String {
    if let Some(path) = overridden {
        return path.to_string();
    }
    let raw = Path::new(env!("CARGO_MANIFEST_DIR")).join("../frust");
    raw.canonicalize()
        .unwrap_or(raw)
        .to_string_lossy()
        .into_owned()
}

/// Infers a project name from `dir`'s basename, resolving `.`/`..`
/// lexically — the target need not exist yet, so this can't use
/// `fs::canonicalize`.
fn infer_project_name(dir: &Path) -> Result<String> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let absolute = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        cwd.join(dir)
    };
    let normalized = normalize_lexically(&absolute);
    normalized
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .with_context(|| {
            format!(
                "`{}` has no basename to use as a project name",
                dir.display()
            )
        })
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
        assert_eq!(
            infer_project_name(Path::new("/tmp/my_app")).unwrap(),
            "my_app"
        );
    }

    #[test]
    fn infer_project_name_resolves_dot_to_cwd_basename() {
        let cwd = std::env::current_dir().unwrap();
        let expected = cwd.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(infer_project_name(Path::new(".")).unwrap(), expected);
    }

    #[test]
    fn infer_project_name_resolves_trailing_parent_dir() {
        assert_eq!(
            infer_project_name(Path::new("/tmp/foo/../my_app")).unwrap(),
            "my_app"
        );
    }

    fn unique_temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-create-cmd-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn base_args(dir: &Path) -> CreateArgs {
        CreateArgs {
            dir: dir.to_string_lossy().into_owned(),
            org: "dev.f0x".to_string(),
            project_name: Some("my_app".to_string()),
            description: "A new Frust application.".to_string(),
            overwrite: false,
            template_dir: None,
            frust_path: None,
            deeplink_scheme: None,
            deeplink_host: None,
        }
    }

    /// Task 07: an invalid `--deeplink-scheme` (a full URL, not a bare
    /// scheme) is rejected before any file is written.
    #[test]
    fn run_rejects_invalid_deeplink_scheme_before_writing_any_file() {
        let dest = unique_temp_dir("invalid-deeplink-scheme");
        let mut args = base_args(&dest);
        args.deeplink_scheme = Some("myapp://".to_string());

        let err = run(args).unwrap_err();
        assert!(err.to_string().contains("--deeplink-scheme"), "{err}");
        assert!(!dest.join("Cargo.toml").exists());

        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn run_accepts_valid_deeplink_scheme() {
        let dest = unique_temp_dir("valid-deeplink-scheme");
        let mut args = base_args(&dest);
        args.deeplink_scheme = Some("myapp".to_string());

        assert!(run(args).is_ok());
        assert!(dest.join("Cargo.toml").exists());
        let manifest =
            std::fs::read_to_string(dest.join("android/app/src/main/AndroidManifest.xml")).unwrap();
        assert!(manifest.contains("android:scheme=\"myapp\""), "{manifest}");

        let _ = std::fs::remove_dir_all(&dest);
    }
}
